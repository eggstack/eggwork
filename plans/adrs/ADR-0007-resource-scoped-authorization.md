# ADR-0007: Resource-Scoped Authorization — Implement the Boundary or Remove the Hook

Status: proposed — **not yet accepted**. This ADR records a decision that has not been made. No implementation work may be handed off on the strength of this document.

Date: 2026-10-23

Decision owners: project maintainers

Related authority:

- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`
- `plans/adrs/README.md`
- `plans/000-long-term-specification.md`
- `plans/security/threat-model.md`
- `architecture/overview.md` — divergence #4
- `plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md` — finding SI-3
- `plans/implementation/security-isolation-resource/005c-resource-scoped-authorization-corrective.md` — the implementation plan, **blocked** on this ADR
- `plans/003-planning-process.md` §4 (dependency discipline) and §8 (architecture changes)

## Context

Eggwork's node service authorizes every request against a per-principal grant list. The grant list is operation-scoped: a principal holds `execute`, `observe`, `blob-read`, and so on.

The server also contains a resource-aware authorization mechanism, and it is fully built. `crates/eggwork-server/src/lib.rs` defines:

- `pub enum ResourceId { Execution, Blob, Workspace, Artifact }` — a typed resource identifier;
- `pub struct OperationRequest { operation, resource: Option<ResourceId> }`;
- `fn resource_for_path(operation, path) -> Option<ResourceId>`, which maps a request path segment to a typed resource;
- the public trait `Authorizer`, whose `fn authorize_request(&self, principal, request: &OperationRequest) -> bool` has a default implementation that delegates to the operation-level `fn authorize(&self, principal, operation) -> bool`;
- `fn authorize_resource(state, principal, operation, resource) -> bool`, the resource-scoped call helper, invoked at six handler sites — the blob-find-missing handler (`BlobRead`/`Blob`), the blob-prepare handler (`BlobWrite`/`Blob`), the workspace-create handler (`WorkspaceCreate`/`Workspace`), the workspace-derive handler (`WorkspaceCreate`/`Workspace`), and the execute handler twice (`Execute`/`Execution`, and `Execute`/`Workspace` when the request names one). The `dispatch` function additionally calls `authorize_request` with a `resource_for_path`-derived resource for every routed request.

The production authorizer does not use any of it. `crates/eggwork-server/src/operations.rs` defines `pub struct GrantAuthorizer(HashMap<PrincipalId, HashSet<Operation>>)` and implements **only** `authorize`. The trait's default `authorize_request` therefore applies on every production path, and the typed `ResourceId` is constructed, passed, and discarded. The only exerciser of the resource-scoped path is a unit test that asserts `resource_for_path` maps a path segment to a `ResourceId`.

This is not a new observation. `architecture/overview.md` records it as divergence #4 — *"`GrantAuthorizer` implements only `Authorizer::authorize`, **not** the resource-aware `authorize_request` hook. Resource-scoped authorization is therefore inert on the real deployment path; ownership is enforced only in the store, by `principal_id` comparison."* — and `plans/security/threat-model.md` states the same as an open remaining boundary for the "Authenticated unauthorized principal" actor: *"Broad BlobRead capability permits any known digest unless resource-scoped policy is configured."*

What is actually enforced today:

- The store's `principal_id` fences, not the authorizer. `ExecutionStore::load_snapshot_for_principal` selects by `(execution_id, principal_id)`; the artifact store's `load` selects by `(artifact_id, principal_id)` and its list by `(execution_id, generation, principal_id)`; the workspace store's `lookup_manifest` selects by `(principal_id, manifest_digest)` and `WorkspaceStore::materialize` rejects a conflicting materialization whose `principal_id` differs.
- Blob storage has **no** owner fence. It is content-addressed and shared, so the same digest is the same bytes for every principal. A principal holding `BlobRead` may read any digest the node knows.

The decision this ADR must make is therefore: **does the node gain a per-resource authorization boundary, or does it lose the appearance of one?**

The decision is durable and consequential in both directions. Implementing the boundary changes what a per-principal grant means and adds a configuration surface. Removing the hook changes the public API of a published crate and removes a mechanism that a future implementer, reviewer, or integrator could reasonably read as available. Leaving it as it is leaves the node presenting a capability it does not have.

## Decision drivers

- **Truthfulness of the advertised surface.** A mechanism that is present but inert must not be represented as available. `AGENTS.md`'s hard invariant "Isolation claims are earned" states the principle for isolation; the same principle applies to authorization.
- **The boundary that is documented.** `plans/security/threat-model.md` is a standing document that states what the node enforces and what it does not. It currently names an open boundary. Leaving that boundary open while a mechanism for closing it sits inert in the tree is the situation this ADR exists to end.
- **Cost proportionality.** The mechanism is already built and publicly exported. Removing it is cheap; implementing it is not.
- **Compatibility.** `ResourceId`, `OperationRequest`, and the `Authorizer` trait are `pub` in `eggwork-server`, a published workspace crate. Removal is a breaking API change.
- **No new authority.** Eggwork must not become a policy engine, a role system, or a multi-tenant IAM product. The decision must stay inside "per-principal grants, optionally narrowed to named resources".
- **Blobs are special.** Content-addressed shared storage has no owner to fence against, so any resource-scoped boundary for blobs must be an explicit identifier list, not an ownership comparison. The decision must be explicit about this rather than discovering it during implementation.
- **Existing deployments must keep working.** A principal with no resource scopes must keep receiving exactly the operation-level decision it receives today.

## Alternatives

### A. Implement resource-scoped grants

`GrantAuthorizer` implements `authorize_request` against a grant configuration that names resources, and the typed `ResourceId` decides.

- The grant configuration gains an optional, additive resource-scope list naming the operation, the resource kind, and the resource identifier. A grant without scopes keeps today's operation-level meaning.
- For `Execution`, `Workspace`, and `Artifact`, the store's existing `principal_id` fence remains authoritative for ownership; the authorizer may narrow, never widen.
- For `Blob`, the scope is an explicit digest list, because there is no owner to compare against.
- The scope applies to the operations the existing call sites already name — `BlobRead`, `BlobWrite`, `WorkspaceCreate`, `Execute` — and the ADR must also settle whether it applies to the operations `resource_for_path` already resolves for `dispatch`: `Observe`, `Cancel`, `Renew`, `Events`, `ArtifactRead`.

**Consequences.** The node gains a real per-resource boundary and the threat model's open boundary can be closed with evidence rather than with a caveat. The cost is a new configuration surface, new validation, new tests, and a per-request decision that consults a set. The blob boundary becomes real rather than notional. The `architecture/overview.md` divergences table gains a closed row.

**Compatibility.** Additive to the configuration surface and to the public API. The one sharp edge is `#[serde(deny_unknown_fields)]` on `ClientGrant`: an older node reading a newer configuration containing a resource scope will refuse to start. That refusal is correct — a node must not silently ignore a scope it does not understand — and must be documented as such.

### B. Remove the hook and the plumbing

Delete the resource-scoped authorization surface so the mechanism stops looking available.

- Delete `pub enum ResourceId` and the `resource` field of `pub struct OperationRequest`.
- Delete `fn resource_for_path` and `fn authorize_resource`.
- Remove the `authorize_request` method from the public `Authorizer` trait and its default implementation.
- Collapse the six resource-scoped handler call sites and the `dispatch`-level call to the operation-level authorizer, and remove the unit test that exercises `resource_for_path`.

**Consequences.** The node stops advertising a boundary it does not enforce, and the divergence-table row can be closed honestly: the boundary that exists is the store's `principal_id` fence, and the blob boundary is exactly what the threat model says it is. The cost is that a per-resource boundary is not available until a future decision builds one, and that a future implementer starts from a smaller tree. The blob boundary remains open and must be stated plainly rather than implied closed.

**Compatibility.** A breaking change to the public API of a published crate. Every in-tree consumer must be enumerated and migrated; the out-of-tree position must be stated honestly rather than assumed; the release version and notes must reflect the removal.

### C. Defer, and mark the mechanism as unavailable

Keep the hook in the tree but make its unavailability discoverable to a caller — for example, through an explicit protocol marker, a capability-advertisement string, or an operator-visible statement that resource-scoped authorization is not in force.

**Consequences.** The smallest change, and it preserves the option to implement later without re-plumbing. But it leaves a public API that still *looks* like a resource-scoped hook, and the marker is only truthful if it is discoverable by the party who would otherwise rely on it. If the marker is a source comment or a documentation note, this alternative collapses into the status quo and does not satisfy the truthfulness driver.

**Compatibility.** Additive.

This alternative is viable **only** if the ADR states concretely how a caller learns that resource-scoped authorization is not in force, and only if the retained hook cannot be mistaken for a working one by a reader of the code. The ADR must record that requirement explicitly if this option is selected.

## Consequences of the decision as a whole

Whichever option is selected:

1. The `architecture/overview.md` divergences table must be updated to the decided disposition. Divergence #4 is either closed with a statement of what now enforces the boundary, or rewritten to describe the mechanism actually in force.
2. `plans/security/threat-model.md`'s remaining-boundary column for the "Authenticated unauthorized principal" row must be updated to state the boundary as it is after the change.
3. `architecture/server-node.md` and `architecture/operations-cli.md` must describe the authorization decision path as it is.
4. Authorization must continue to precede resource resolution, and a denied caller must continue to be unable to distinguish an existing resource from an absent one.
5. The store's existing `principal_id` fences must not be weakened.
6. The configuration loader must continue to fail closed on a malformed or unknown field.

Option A additionally narrows what a grant means; Option B additionally removes a public API; Option C additionally commits the project to making an unavailability discoverable.

## Compatibility implications

- **Option A** is additive at the wire and API level. A grant without scopes behaves identically to today. The `deny_unknown_fields` interaction with an older node reading a newer configuration is a deliberate refusal and must be documented.
- **Option B** is a breaking public-API removal in `eggwork-server`. The removal must be reflected in the release version and notes and must not be bundled silently into a release that claims to be a qualification-only candidate.
- **Option C** is additive, but it commits the project to an explicit unavailability marker, whose shape is part of the decision and must be specified here.
- No option may change the wire representation of a request or response type without a decision recorded in this ADR and coordination with the control-plane shared-protocol contract corrective, which owns `API_SCHEMA_VERSION`.

## Verification expectations

Common to all options:

- the divergences-table row and the threat-model row match the code;
- the existing authorization test matrix — the operation-level grant matrix, the "request payload cannot supply authoritative principal" test, and the routing test — is green and unchanged;
- a denied caller still cannot distinguish an existing resource from an absent one, and the refusal still occurs at the same point in the dispatch path;
- the store's `principal_id` fences are unchanged and still refuse a foreign-owned resource;
- the configuration loader still refuses a malformed or unknown field, and the node still refuses to start;
- the full pre-submit gate is green on the reviewed head.

Option A additionally:

- a resource scope measurably narrows an operation grant — a test that fails if the scope is ignored, so that the hook cannot land inert a second time;
- the owner fence holds for `Execution`, `Workspace`, and `Artifact`, proven by a test that grants a scope and then attempts to reach a foreign-owned resource;
- a blob scope is an explicit digest list and does not permit a digest outside it;
- a principal with no scopes receives exactly the operation-level decision;
- every configuration error class — unknown operation, unknown resource kind, malformed identifier, a scope for an unheld operation, a duplicate scope, an over-cap count — refuses the configuration at load time.

Option B additionally:

- an assertion that `ResourceId`, `OperationRequest.resource`, `resource_for_path`, `authorize_resource`, the `authorize_request` trait method, and the unit test are gone, and a test that fails if any of them is reintroduced;
- an assertion that no handler-level call constructs a typed resource;
- the full behaviour of the authorization path is unchanged.

Option C additionally:

- a test proving that the unavailability marker is produced on the path where a caller would rely on resource scoping, and that it is discoverable without inspecting the node's source.

## Architecture review

**Yes — this decision class requires architecture review**, and this ADR is that review's vehicle.

`plans/003-planning-process.md` §8 requires an ADR or a canonical amendment when implementation discovers that a durable invariant or ownership boundary must change. This decision changes a public API shape (Option B), adds a configuration and policy surface (Option A), or commits the project to a durable "not available" statement (Option C). Each of those constrains later implementation beyond this milestone and is therefore the kind of decision an ADR exists to record.

The §8 examples of changes requiring architecture review include *"changing required isolation into silent best-effort"* — the same class of concern as this one. A required guarantee that is silently absent is the pattern both branches are designed to eliminate: Option A makes the guarantee real, Option B makes its absence explicit, and Option C makes its absence discoverable. All three are legitimate; the review's job is to pick one and write it down, not to leave three plausible.

The review must also confirm that the chosen option does not move Eggwork toward owning policy, roles, or multi-tenant IAM, and that it does not weaken the store's ownership fences.

## Decision

**Not yet made.** This ADR is `proposed`.

When the decision is made, this section is completed with the selected option, the decision date, and the constraints that apply to it, and the `Status:` line above is changed from `proposed` to `accepted`. Until then:

- `plans/implementation/security-isolation-resource/005c-resource-scoped-authorization-corrective.md` remains `blocked`, with this ADR named as its concrete blocker;
- per `plans/003-planning-process.md` §4, that plan must not be handed off as ready merely because adjacent implementation exists;
- the implementation plan's Branch A (§5) and Branch B (§6) are written to be executable without re-deriving the design, so that whichever option is selected, the implementation is mechanical.

## Supersession

None. This ADR augments, and does not replace, ADR-0001 or ADR-0005. If a later decision changes the outcome, this ADR is superseded by a new ADR rather than rewritten.
