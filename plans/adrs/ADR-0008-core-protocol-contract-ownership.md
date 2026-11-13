# ADR-0008: Core Ownership of the Protocol Contract Facts

Status: accepted

Date: 2026-10-23

Decision: `eggwork-core` owns the control-plane API schema version, the
`exec.argv.v1` capability feature name, and the terminal/non-terminal
classification of `ExecutionState`. The runner keeps producer ownership of the
capability feature names it produces and of the sandbox profile name. No wire
value, wire field, or capability byte changes.

Related authority:

- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`
- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `architecture/core-domain.md` — the **States** section, which currently records
  that `eggwork-core` owns no terminal concept
- `architecture/overview.md` — divergence #12
- `plans/subsystems/control-plane-protocol-shared-contract-corrective-addendum.md` —
  the corrective that owns the findings and the ownership decision
- `plans/implementation/control-plane-protocol/004-shared-protocol-contract-corrective.md` —
  the implementation plan, whose §12.3 stop condition requires this record
- `plans/003-planning-process.md` §8 (architecture changes)

## Context

Three protocol contract facts were each stated by more than one owner, and the
copies had no mechanical relationship. Both sides of every pair currently agree
by value, so none of this is a live bug. The defect is structural: a future
rename or bump on the producing side desynchronises the consuming side
silently, and neither the compiler nor the test suite objects.

1. **The control-plane API schema version.** The client and the server each
   declared a private `API_SCHEMA_VERSION`. The client stamps its value into
   the execute, control, workspace-create, and workspace-derive request bodies.
   The server compares the received value against its own and rejects a
   mismatch with HTTP 426 `protocol_version`. Nothing linked the two
   declarations.

2. **The `exec.argv.v1` capability feature name.** The server produces the
   name in its static capability advertisement; the client consumes it when it
   checks that a node is protocol-compatible. Each endpoint crate spelled the
   literal independently.

3. **The terminal/non-terminal classification of `ExecutionState`.**
   `eggwork-core` defines `ExecutionState` as a flat nine-variant enum and
   owned no terminal concept at all: no predicate, no grouping, no canonical
   spelling. The server therefore enumerated the classification approximately
   nine times — in the store's state-name and terminal-name helpers, in an
   inline match in the same store file that did not call its own helper, in a
   separate terminal predicate, in an inline artifact-capture eligibility
   subset, in a wildcard-terminated terminal-to-metric-name match, in a
   non-terminal match in the operations module, in a parallel SQL literal list,
   and in a second enum-to-string map for display labels.

`eggwork-core` already owned the closest precedent: it exports
`NODE_TELEMETRY_SCHEMA_VERSION`, consumed by the client's telemetry reader, the
server's telemetry projection, and core's own `NodeTelemetry::validate`.

`plans/003-planning-process.md` §8 requires that a durable ownership-boundary
change be recorded as an ADR or a canonical amendment before the implementing
work lands. This ADR is that record.

## Alternatives

A. **The server owns the schema version and the feature names; the client
   imports them.** Rejected. `crates/eggwork-client` depends on `eggwork-core`
   only. The client cannot depend on the server without inverting the
   workspace's one-way dependency rule, and the server cannot be depended upon
   by the client.

B. **A new workspace crate owning the protocol contract.** Rejected. It would
   add a dependency edge to a five-crate workspace whose dependency graph is
   deliberately flat and one-way, and it would move wire types away from the
   crate that already models them. The facts have exactly one consumer set
   that spans endpoints, and the crate both endpoints already share is that
   set's natural home.

C. **Leave the literals in place and pin them with tests only.** Rejected. A
   test pins a value at the moment it is written; it does not make a future
   one-sided edit a compile error, and the terminal-state case has nine
   enumeration sites rather than two. The classification in particular cannot
   be pinned by value, because the defect is that the set is written nine
   times.

D. **`eggwork-core` owns the contract facts that both endpoints consume; the
   producing crate owns the names it alone produces.** **Selected.**

## Decision and constraints

1. **`eggwork-core` owns the control-plane API schema version.** It is
   declared exactly once, publicly, in `crates/eggwork-core/src/lib.rs`. The
   client and the server import it; neither keeps a private copy. The server's
   check sites, and when it checks, are unchanged.

2. **`eggwork-core` owns the `exec.argv.v1` capability feature name.** The
   server produces the name and the client consumes it, and the client cannot
   depend on the server. `eggwork-core` is the crate both already depend on,
   and it already owns the `NodeCapabilities` type whose `features` field
   carries the name.

3. **`eggwork-core` owns the terminal/non-terminal classification of
   `ExecutionState`**, together with the canonical wire spelling of a state
   and a way to enumerate every variant. The classification is an exhaustive
   match over all nine variants with no wildcard arm, and the wire-spelling
   function is likewise exhaustive. Adding a tenth variant is therefore a
   compile error in `eggwork-core` rather than a silent behaviour change in a
   downstream crate.

4. **The artifact-capture eligibility subset stays a distinct policy.** The
   subset that admits artifacts for capture is exactly `Succeeded` and
   `Failed`. It is not the terminal set and must not be folded into it, nor the
   terminal set folded into it.

5. **Producer ownership is preferred where it is available.** The runner keeps
   ownership of the capability feature names it produces — the Landlock
   workspace-rw name and the three `resources.cgroups-v2.*` names — and of the
   sandbox profile name. The server imports the runner's constants rather than
   restating them.

6. **The sandbox helper keeps no workspace-crate dependency.** The helper is
   the trusted artifact and its dependency surface is part of its trust story,
   so the helper's copy of the profile name is pinned by a test rather than by
   a shared crate. The same rule applies to the protocol limit constants the
   helper restates.

7. **No wire value changes.** The schema version stays `1`. Every capability
   feature name keeps its exact bytes. `ExecutionState` keeps its derived
   `Serialize`/`Deserialize` with no rename attribute and stays externally
   tagged PascalCase. The server's stored `state` column keeps the same
   PascalCase strings, and the operator-facing state labels keep the same
   snake_case output.

8. **`eggwork-core` keeps no internal dependency.** The crate hierarchy stays
   one-way: core and the sandbox helper depend on nothing internal; the client
   depends on core; the server depends on core and the runner; the runner
   depends on core; the client is a dev-dependency of the server only.

9. **No authentication, authorization, lease, generation, fencing, digest, or
   idempotency semantics change.** This ADR moves where a fact is written, not
   what it is.

10. **Sharing a constant prevents creating skew inside one build.** It cannot
    remove skew between two already-deployed binaries. A future schema-version
    bump needs its own compatibility story; this ADR does not pre-authorize
    one.

## Consequences

- A change to the API schema version, to the `exec.argv.v1` feature name, or
  to the terminal-state classification becomes a compile-time or
  test-detectable event rather than a silent divergence.
- The server's nine terminal-state enumeration sites collapse to derivations
  from one owner. The metric-name mapping becomes total by construction, so a
  newly added terminal variant cannot silently lose its metric name.
- The active-execution count query derives its SQL literal list from the
  shared spelling. That list becomes runtime-assembled, so the query must stay
  parameterised; string-concatenated SQL is not acceptable on that path.
- `architecture/core-domain.md` currently states that `eggwork-core` has no
  `is_terminal()` helper and no terminal-state grouping. That statement becomes
  false when this decision is implemented and must be rewritten in the same
  pass. It is an update to a recorded observation, not a deletion.
- Sharing a constant within one build is not a migration story: no stored
  format moves, no rollout step is needed, and no coordination with deployed
  clients or nodes is required, because no observable value changes.

## Scope boundary

This ADR records an ownership decision. It does not specify the implementation
of that ownership, the tests that prove it, or the milestone that delivers it;
those belong to
`plans/implementation/control-plane-protocol/004-shared-protocol-contract-corrective.md`
and its closure record.
