# Workspace and Artifact Transport Post-Closure Corrective — Data-Plane Ownership and Fail-Closed Capture

Status: active corrective; M005a proposed, M005b proposed

Predecessor roadmap:

- `plans/subsystems/workspace-artifact-transport-roadmap.md`

Historical predecessor closures:

- `plans/closure/workspace-artifact-transport/001-status.md` — blob store and digest protocol
- `plans/closure/workspace-artifact-transport/002-status.md` — manifest and safe materialization
- `plans/closure/workspace-artifact-transport/003-status.md` — declared artifacts, retention, and GC
- `plans/closure/workspace-artifact-transport/004-status.md` — reusable manifest CAS and derived materialization

Canonical authority remains:

- `plans/000-long-term-specification.md#11-workspace-and-artifact-model`
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`
- `plans/002-long-term-roadmap.md`
- `plans/003-planning-process.md#corrective-work`

Finding source:

- repository-wide code interrogation pass over the reviewed baseline `81f34e0`; every finding below is new and is **not** recorded in the `architecture/overview.md` divergences table.

## 1. Findings

The workspace and artifact transport subsystem is three crate-private modules inside
`eggwork-server` — `blob.rs`, `workspace.rs`, and `artifact.rs` — that own immutable
content-addressed bytes, the reconstruction of a portable tree from a manifest, and the capture of
an execution's declared outputs. The subsystem's stated discipline is that a fact has exactly one
owner and every other module consults that owner.

All four findings below are the same defect at four different scales: **a data-plane fact is
asserted in more than one place, and the copy that is not the owner is the copy an operator or a
caller can observe.** In WA-1 the duplicated fact is "capture succeeded". In WA-2 it is "this blob
is a GC candidate". In WA-3 it is "this is where a blob lives on disk". In WA-4 it is "this is the
owner id under which a manifest's blobs are pinned".

Two of the four are violations of invariants the repository has already written down, in the very
module that owns them. The corrective therefore restores stated rules; it does not invent new
policy.

### 1.1 On a non-Unix target, declared-artifact capture is refused but the execution still reports success (WA-1, highest severity)

**What the data plane is supposed to guarantee.** The product's top-level claim is terminal state
*plus declared artifacts*. Declared outputs are the default artifact capture boundary
(`plans/subsystems/workspace-artifact-transport-roadmap.md`, durable invariant 6), and the data
plane is fail-closed by design: `architecture/data-plane.md` states in its shared-discipline
section that there is no best-effort path, that a symlinked output is refused rather than copied,
and that a missing required output fails the whole capture instead of producing a partial record
set. A caller that declared outputs and received none must be able to tell that apart from a
caller that declared no outputs.

**What the code actually does.** `crates/eggwork-server/src/artifact.rs` has two variants of
`open_file_beneath`. The unix variant opens the root and each path component
descriptor-relatively with `O_NOFOLLOW`. The `#[cfg(not(unix))]` variant returns
`io::ErrorKind::Unsupported` with the message *"safe artifact capture is unavailable on this
platform"*. `capture_blob` in the same module calls `open_file_beneath` and propagates the error
with `?`; `capture_declared` calls `capture_blob` inside its capture loop and propagates with `?`
as well, so the first failure aborts the entire capture and no artifact record is published.

`crates/eggwork-server/src/lib.rs` (`run_execution`) calls `artifact::capture_declared` in the
execution-finalization path and matches on the result: `Ok(count)` sets
`execution_result.artifact_count`, and `Err(_)` sets
`execution_result.finalization_failure = Some(ExecutionFinalizationFailure::ArtifactCapture)`.
Nothing else changes. The execution's own state remains `Succeeded`, its `exit_code` remains the
target's, and `artifact_count` remains `0`. The terminal event published afterwards is
`ExecutionEventKind::State(terminal)`; `eggwork-core`'s `ExecutionEventKind` has no
artifact-specific variant, so the failure is not in the event stream either.

The consequence is a **successful execution with zero artifacts and a buried marker**. This is
not an untested edge: the integration test in `crates/eggwork-server/src/lib.rs`
(`blob_protocol_streams_verifies_deduplicates_and_enforces_quota`) asserts exactly this
combination — `state == ExecutionState::Succeeded`, `exit_code == Some(0)`,
`artifact_count == 0`, `finalization_failure == Some(ExecutionFinalizationFailure::ArtifactCapture)`.
The current behaviour is therefore *asserted*, not merely tolerated.

Two further facts sharpen the consequence. First, `static_capability_features` in
`crates/eggwork-server/src/lib.rs` advertises `"artifact.declared.v1"` on every platform with no
platform condition, so a Windows node advertises declared-artifact capture and then refuses the
capture — a static claim where the repository's own capability rule requires a runtime probe.
Second, `ExecutionEventKind::Diagnostic` is the protocol's only carrier for a non-fatal
explanation, and it currently has **no production publisher**: every use of the variant in the
repository is in test code. There is therefore no existing production path by which the capture
failure could reach an event-streaming caller at all.

`x86_64-pc-windows-msvc` is a published release target (`README.md`, "Installing a published
release"), so this is the disposition of a shipped platform, not a theoretical one.

**Why this is not already recorded.** `architecture/data-plane.md` documents the behaviour
accurately — its error-model section says that `capture_declared` has no partial-success result
and that the caller records `ExecutionFinalizationFailure::ArtifactCapture` "while leaving the
execution's own exit state intact", and its "Honest gaps" section states that no test exercises
the non-unix branches, naming `open_file_beneath` returning `ErrorKind::Unsupported`. The design
record is honest about the mechanism. What is missing is the *judgment*: the divergences table in
`architecture/overview.md` records row 18 (artifact GC does not release blob references) but has
no row for this, and the README's Windows caveat covers isolation, resource enforcement, and
service management while saying nothing about artifacts. A caller reading the README would
reasonably expect declared artifacts to work on a Windows node.

**Severity.** Highest of the four. Every caller-declared output is silently lost on a shipped
platform, and the loss is reported as success.

### 1.2 The operator's GC preview and the apply pass use different candidate predicates (WA-2)

**What the data plane is supposed to guarantee.** The canonical blob candidate selection lives in
`crates/eggwork-server/src/blob.rs`, in `BlobStore::unreferenced_candidates`. Its selection is
`WHERE NOT EXISTS (SELECT 1 FROM blob_references r WHERE r.digest = blobs.digest)`, ordered
coldest-first and bounded by `limit`. `BlobStore::garbage_collect` evaluates that same statement
in both its dry-run and its real pass. The doc comment on `unreferenced_candidates` states the
rule explicitly:

> *"Shared by the dry run and the real run on purpose. They select at *different points* in the
> transaction — a preview before expired blob references are deleted, the real pass after — so the
> same statement must be evaluated in both places. Two hand-copied copies of it are what let the
> two paths drift apart and made `NodeGcReport.blob_candidates` under-predict what `--apply` would
> actually remove."*

`architecture/data-plane.md` records the same reason independently: *"Reclamation queries
deliberately use `NOT EXISTS (SELECT 1 FROM blob_references ...)` rather than the counter."* The
counter is a denormalization: `blobs.reference_count` is written only by the
`blob_reference_insert` / `blob_reference_delete` triggers, never by Rust.

**What the code actually does.** `crates/eggwork-server/src/operations.rs` does not use that
statement. Its `dry_run_gc` re-implements the blob candidate query by hand as
`WHERE reference_count = 0` — a different predicate over a different column, against a different
source of truth than the one the owner module selected on purpose. `operations::collect_garbage`
routes `dry_run == true` to `dry_run_gc`, and the apply path opens the real `WorkspaceManager`,
`BlobStore`, and `ArtifactStore` and calls their own collectors. `crates/eggwork-server/src/bin/eggworkd.rs`
makes the preview the default for the `gc` verb (`let dry_run = !args[index..].iter().any(|arg| arg
== "--apply");`), so the divergent path is the one an operator reaches first.

The same function hand-copies the workspace candidate SQL, the retained-manifest candidate SQL,
and the artifact candidate SQL verbatim from their owner modules, and re-derives the store
metadata filenames (`<blob_root>/metadata.sqlite`, `<workspace_root>/metadata.sqlite`) that
`blob.rs` and `workspace.rs` own.

**The consequence.** The preview and the apply pass disagree about what is collectable, and the
disagreement is invisible in both directions. A blob whose `reference_count` is `0` while a
`blob_references` row lingers is predicted for deletion by the preview and not removed by
`--apply`. A blob whose `reference_count` is positive while no reference rows exist is not
predicted and *is* removed by `--apply`. Which way a given store errs depends on whether its
trigger-maintained counter has drifted from its reference rows — a property the preview depends
on and the apply path does not. Either way the operator's preview is a guess about a different
question than the one `--apply` answers.

The existing preview test makes the drift structural rather than accidental: the fixture in
`crates/eggwork-server/src/operations.rs`
(`garbage_collection_preview_reads_only_and_reports_bounds`) builds a synthetic blob database
whose `blobs` table has columns `(reference_count, size_bytes, last_used_unix_ms)` — **no `digest`
column** — so the canonical predicate cannot even be evaluated against it. The fixture was shaped
to the hand-copied query.

This is a violation of a stated invariant, not a hypothetical: the rule is written on the owner
module and broken in the sibling module. The historical claim that is no longer sufficient is in
`plans/closure/workspace-artifact-transport/004-status.md`, which records that the collector
ordering is "in both `NodeServer::collect_garbage` and the operator `collect_garbage` path, with
matching dry-run previews". Per `plans/003-planning-process.md#corrective-work`, that closure
record stays immutable and this corrective owns the newly discovered defect.

### 1.3 The blob store's on-disk path layout is re-derived outside the `BlobStore` abstraction (WA-3)

**What the data plane is supposed to guarantee.** `crates/eggwork-server/src/blob.rs` owns the
shard layout in exactly one place: `BlobStore::path_for` returns
`root.join(&digest.as_str()[..2]).join(digest.as_str())`. `architecture/data-plane.md` describes
that layout as a single level of sharding on the first two hex characters and states the reason
it is a pure function of the digest: a digest that survives `BlobDigest::parse` cannot name a
location outside the blob root.

**What the code actually does.** `crates/eggwork-server/src/operations.rs` re-derives the same
layout by hand inside the operator's blob inspection verb (`inspect_blob`): it joins
`config.blob_root` with `&digest.as_str()[..2]` to form the shard directory, then joins the full
digest to form the object path. `crates/eggwork-server/src/workspace.rs` reaches the same place
from the other direction, in `materialize_tree`, by calling `blobs.path_for(digest)` and then
opening the result with a plain `File::open`, accepting the bytes on a **size-only** check
(`io::copy` must yield exactly `size_bytes`) and bypassing the store's verified-open path
(`BlobStore::open_verified`, which calls `BlobStore::verify_existing` and re-hashes the whole
object against its digest).

**The consequence.** Two consumers of the layout sit outside the abstraction that owns it. A
change to shard depth or file naming would silently break the operator inspection verb, which
would report `NotFound` for objects that exist, and would not be caught by any test that exercises
only the store. The workspace materializer's bypass is narrower but real: `materialize` does
verify every referenced blob in its pre-flight via `blobs.verify_existing`, but `materialize_tree`
re-reads the object afterwards and copies it without re-verifying, so the bytes a workspace
actually receives are checked only by length.

### 1.4 The manifest owner-id composite format is inlined at two sites instead of using its own helper (WA-4)

**What the data plane is supposed to guarantee.** The `blob_references.owner_id` for owner kind
`"manifest"` is a `"{principal}:{manifest_digest}"` composite.
`crates/eggwork-server/src/workspace.rs` defines exactly one producer for it:
`fn manifest_owner_id(principal_id: &PrincipalId, manifest_digest: &str) -> String`.
`architecture/data-plane.md` records the composite as part of the reference-accounting contract.

**What the code actually does.** Two call sites do not call it. In
`WorkspaceManager::manifest_garbage_collect` and in `WorkspaceManager::reconcile_manifests`, the
`principal_id` value is read out of a SQL row as a `String`, and the composite is built inline as
`format!("{principal_id}:{manifest_digest}")`. The helper takes `&PrincipalId`, and
`PrincipalId::new` returns a `Result`, so at these sites there is no validated `PrincipalId` to
hand it — which is why the format was inlined rather than reused.

**The consequence.** The format has two producers. If the composite ever changes, the two inlined
sites release blob references under an owner id that does not match the one the references were
created under. The result is orphaned pins with no error and no symptom other than a blob
reference count that never falls — bytes that the store will not reclaim and that no operator
report explains.

## 2. Corrective milestones

### M005a — Declared-artifact capture fails closed

Class: invariant

Status: proposed

Implementation plan:

- `plans/implementation/workspace-artifact-transport/005a-declared-artifact-capture-fail-closed-corrective.md`

Objective:

Make the declared-artifact failure mode honest. An execution whose declared outputs could not be
captured must not present as a successful execution with missing declared artifacts, and the
platform claim must match the code on every shipped target.

### M005b — GC preview and apply share one implementation; store-owned formats have one producer

Class: invariant

Status: proposed

Implementation plan:

- `plans/implementation/workspace-artifact-transport/005b-gc-preview-and-store-ownership-corrective.md`

Objective:

Make the operator's GC preview and its apply pass select candidates through the same
store-owned statement, and bring the blob path layout and the manifest owner-id composite back
under the modules that own them.

## 3. The data-plane ownership rule this corrective establishes

One owner per fact. After M005a and M005b, each of the following facts is produced in exactly one
place, and every other place consumes it rather than restating it.

| Fact | Sole owner | Consumers must do this |
|---|---|---|
| "Was the declared capture completed?" | `crates/eggwork-server/src/artifact.rs` (`capture_declared` and its result type) | The server's finalization must consume the result and translate it into a truthful terminal state; it must not infer success from the absence of an error |
| "These blobs are GC candidates" | `crates/eggwork-server/src/blob.rs` (`unreferenced_candidates`) | The operator preview and the operator apply pass must evaluate that same statement; no module may write its own candidate query |
| "This is where a blob lives on disk" | `crates/eggwork-server/src/blob.rs` (`BlobStore::path_for`) | The operator inspection verb must go through the store; the materializer must go through the store's verified-open path |
| "This is the manifest owner id" | `crates/eggwork-server/src/workspace.rs` (`manifest_owner_id`) | Every site that pins or releases manifest blob references must call the helper |

Two structural rules follow:

1. A fact that a caller or an operator can observe may not be re-derived outside its owner. A copy
   that is only *maintained* by a module's own tests is still a copy.
2. A stated invariant that lives in a doc comment on the owner module is not satisfied by a
   sibling module that behaves differently. "The same statement must be evaluated in both places"
   is a testable claim, and M005b makes it one.

## 4. Durable invariants

These extend the subsystem's existing invariants 1 ("Blob bytes must match their digest"), 6
("Declared outputs are the default artifact capture boundary"), and 7 ("Storage exhaustion
produces typed failure and bounded cleanup").

1. An execution whose declared outputs could not be captured MUST NOT present as a successful
   execution with missing declared artifacts.
2. A capture failure is visible on the terminal state, in the event stream, and to the caller —
   not only as a nested marker inside the terminal snapshot.
3. The relationship between the number of declared outputs and the number of captured artifacts
   is explicit in the terminal result, so a caller can distinguish "no declared outputs" from
   "declared outputs that vanished".
4. The platform claim in `README.md` and in `architecture/overview.md`'s divergences table matches
   the code on every shipped target.
5. The advertised capability set agrees with the runtime fact: a node that cannot capture does not
   advertise that it can. A capability string is a runtime probe, not a compile target.
5. GC candidate selection has one owner. The operator's preview and the operator's apply pass
   evaluate the same statement and cannot diverge by construction.
6. A reclamation query never consults the denormalized `blobs.reference_count` counter in place
   of the `blob_references` rows.
7. The blob path layout has one owner. No module outside `blob.rs` reconstructs it.
8. The manifest owner-id composite has one producer. No module formats it inline.
9. Blob bytes are verified against their digest on the path that materializes them, not only in
   a pre-flight that a later read can race.
10. No authentication, lease, generation, fencing, digest, or bound semantics weaken.
11. Eggwork remains a fixed-target, scheduler-free fabric: no queue, no worker selection, no
    priority or fairness policy, no DAG, and no semantic retry is introduced.
12. The historical M001-M004 closure records remain immutable; this corrective owns the newly
    discovered defects.

## 5. Compatibility and migration

**M005a.** The capture result type is internal to `eggwork-server`. Making the failure
truthful can be done with no wire change at all: the execution's terminal `ExecutionState` and
`ExecutionFailure` already have a vocabulary that can express "the target ran and the declared
outputs could not be collected". If the plan chooses instead to add a declared-output count to
`ExecutionResult`, that is a wire-additive change to a struct that is serialized into the
terminal snapshot; it must carry `#[serde(default)]` so an older peer still decodes it, and the
plan must record that the terminal snapshot is not byte-identical across the change. No stored
format changes. No stored blob, workspace, artifact, or manifest row is rewritten.

**M005b.** No wire protocol change. No stored format change: the blob metadata schema, the
workspace metadata schema, the artifact schema, the `retained_manifests` schema, and the
`"{principal}:{digest}"` composite are all unchanged. The `path_for` layout is unchanged. The
corrective changes only *who computes* these values, not what they are. A blob store whose
`reference_count` has drifted from its reference rows is not migrated by this corrective; the
corrective simply stops the preview from depending on the drift.

**Both.** Neither milestone changes the `eggworkd` command surface, the `NodeGcReport` shape, the
`NodeCapabilities` feature set, or any route.

## 6. Downstream relationships

- **Control plane / execution lifecycle.** The artifact capture failure path lives in the server's
  execution-finalization code (`crates/eggwork-server/src/lib.rs`, `run_execution`), so WA-1
  touches the execution lifecycle that the foundation and control-plane subsystems own. The
  boundary is respected as follows: **the data plane owns capture** — `artifact.rs` owns what
  "capture completed" means and must report it; **the server's finalization owns the truthful
  terminal state** — `run_execution` owns the transition to terminal and owns the mapping from a
  capture result to an `ExecutionState`/`ExecutionFailure`/event. The data plane does not decide
  the execution's terminal state, and the finalization does not re-derive whether capture
  succeeded. Neither milestone changes admission, lease renewal, generation fencing, or the
  event journal's shape.
- **Operations and distribution.** M005b changes the operator's `gc` preview output only in that
  it becomes correct. `docs/operations.md` describes `gc` as a read-only preview by default and
  states that `--apply` takes an exclusive lock on node state; that behaviour is unchanged.
  Operations M007 (the Windows service host) is independent of both milestones.
- **Security, isolation, and resource.** Independent of the data plane itself, but not
  independent of M005a in one narrow place: `ExecutionEventKind::Diagnostic` has no production
  publisher, and the Security M005a degraded-isolation observability corrective needs one for a
  different failure. Both milestones must extend the same server-side event-publishing path
  rather than grow two divergent publishers for the same event kind. Neither milestone touches
  Landlock, cgroups, the helper's trust, or admission.
- **Foundation and execution core.** Independent. The runner's process ownership, timeout,
  cancellation, and output-limit behaviour are unchanged. M005a does not change which executions
  are eligible for capture; it changes what an eligible execution reports when capture fails.
- **CodeGG integration.** Must not claim declared-artifact support on a platform where capture is
  unavailable until M005a closes and the corrected node is re-qualified against it.
- **`NodeServer::collect_garbage`.** The in-process live collector and the operator's offline
  collector are two paths to the same stores. M005b covers the operator's preview/apply pair.
  The live collector's single caller is a test, and the divergence between the two paths is
  noted in the M005b plan as an in-scope simplification opportunity, not as a separate milestone.

## 7. Exit conditions

M005a closes only when:

- no execution can reach terminal state `Succeeded` while a declared output it asked for was not
  captured;
- the capture failure is visible on the terminal state, in the event stream, and through the
  client's own view of the execution — not only as `finalization_failure`;
- the terminal result makes the declared-output count and the captured-artifact count comparable,
  so "no declared outputs" and "declared outputs that vanished" are distinguishable;
- the existing integration test that asserts `Succeeded` + `artifact_count == 0` +
  `finalization_failure == Some(ArtifactCapture)` has been changed to assert the corrected
  disposition, and the replacement test is proven non-vacuous;
- a test of the corrected behaviour is proven to run on a non-Unix target, and the platform
  evidence is recorded against a real Windows host, not inferred from a compile;
- the README's Windows caveat and the `architecture/overview.md` divergences table state the
  artifact disposition for every shipped platform, and the statement matches the code;
- the advertised capability set agrees with the runtime fact on every shipped platform — a node
  that refuses declared outputs does not advertise `"artifact.declared.v1"`;
- `architecture/data-plane.md`'s "Honest gaps" entry about the untested non-unix branches is
  updated to match the new evidence;
- no high or medium finding remains without an explicit owner.

M005b closes only when:

- the operator's dry-run report and the operator's apply pass select candidates through the same
  store-owned statement, so that divergence is structurally impossible;
- the owner-module rule recorded on `BlobStore::unreferenced_candidates` is enforced by a test
  rather than stated only in a doc comment;
- the operator's blob inspection verb resolves object paths through the store rather than
  re-deriving the shard layout;
- the workspace materializer obtains blob bytes through the store's verified-open path rather
  than a size-only check;
- the manifest owner-id composite has exactly one producer and both inlined call sites use it,
  with the `&PrincipalId` versus `&str` mismatch resolved by an explicit decision rather than
  assumed away;
- a test proves that the set of candidates the dry run reports is a subset of — and for the same
  inputs, equal to — the set the apply pass removes;
- no public protocol surface and no stored format changed;
- `architecture/data-plane.md` and `architecture/operations-cli.md` are updated so their
  descriptions of the preview and of blob inspection match the code, and their stale line-number
  citations are replaced with symbol references in the same pass;
- no high or medium finding remains without an explicit owner.
