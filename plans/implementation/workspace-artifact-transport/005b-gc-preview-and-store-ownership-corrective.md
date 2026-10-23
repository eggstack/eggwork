# Workspace and Artifact Transport M005b — GC preview/apply parity and store-owned formats

Status: proposed

Class: invariant

Closure record: `plans/closure/workspace-artifact-transport/005b-status.md`

Source authority:

- `plans/subsystems/workspace-artifact-transport-data-plane-corrective-addendum.md`, findings 1.2 (WA-2), 1.3 (WA-3), and 1.4 (WA-4);
- `plans/subsystems/workspace-artifact-transport-roadmap.md`, durable invariants 1 ("Blob bytes must match their digest"), 3 ("Materialization cannot escape the node-owned root"), and 7 ("Storage exhaustion produces typed failure and bounded cleanup");
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`;
- `plans/003-planning-process.md#verification-philosophy` and `#corrective-work`.

## 1. Objective

Make the operator's GC preview and its apply pass share one implementation, and bring the
store-owned path layout and owner-id formats back under the modules that own them.

Three properties follow, and all three are required:

1. The dry-run report and the apply pass select candidates through the **same** store-owned
   statement or query, so that preview and apply cannot diverge by construction.
2. The operator's blob inspection path goes through the store's path/ownership API rather than
   re-deriving the shard layout, and the workspace materializer obtains blob bytes through the
   store's verified-open path rather than a size-only check.
3. The manifest owner-id composite has exactly one producer, and the two inlined call sites use
   it.

This plan changes no public protocol surface and no stored format. It changes only *who computes*
these values, not what they are.

## 2. Baseline

### 2.1 The candidate-selection divergence (WA-2)

The canonical blob candidate selection lives in `crates/eggwork-server/src/blob.rs`, in
`BlobStore::unreferenced_candidates`. Its predicate is

```sql
SELECT digest, size_bytes FROM blobs
WHERE NOT EXISTS (SELECT 1 FROM blob_references r WHERE r.digest = blobs.digest)
ORDER BY last_used_unix_ms ASC LIMIT ?1
```

`BlobStore::garbage_collect` evaluates that statement in both its dry-run and its real pass. The
doc comment on `unreferenced_candidates` states the rule and the reason it exists:

> *"Shared by the dry run and the real run on purpose. They select at *different points* in the
> transaction — a preview before expired blob references are deleted, the real pass after — so the
> same statement must be evaluated in both places. Two hand-copied copies of it are what let the
> two paths drift apart and made `NodeGcReport.blob_candidates` under-predict what `--apply` would
> actually remove."*

`architecture/data-plane.md` records the same reason independently: *"Reclamation queries
deliberately use `NOT EXISTS (SELECT 1 FROM blob_references ...)` rather than the counter."* The
counter is a denormalization — `blobs.reference_count` is written only by the
`blob_reference_insert` and `blob_reference_delete` triggers, never by Rust.

`crates/eggwork-server/src/operations.rs` does not use that statement. `dry_run_gc` re-implements
the blob candidate query by hand as `WHERE reference_count = 0`: a different predicate over a
different column, against a different source of truth than the one the owner module selected on
purpose. `operations::collect_garbage` routes `dry_run == true` to `dry_run_gc`, and the apply
path opens the real `WorkspaceManager`, `BlobStore`, and `ArtifactStore` and calls their own
collectors. `crates/eggwork-server/src/bin/eggworkd.rs` makes the preview the default for the `gc`
verb (`let dry_run = !args[index..].iter().any(|arg| arg == "--apply");`), so the divergent path
is the one an operator reaches first.

The same function hand-copies the workspace candidate SQL, the retained-manifest candidate SQL,
and the artifact candidate SQL verbatim from their owner modules, and re-derives the store
metadata filenames (`<blob_root>/metadata.sqlite`, `<workspace_root>/metadata.sqlite`,
`<database_path>.artifacts.sqlite`) that `blob.rs`, `workspace.rs`, and `artifact.rs` own.

The preview is wrong in both directions. A blob whose `reference_count` is `0` while a
`blob_references` row lingers is predicted for deletion and not removed. A blob whose
`reference_count` is positive while no reference rows exist is not predicted and *is* removed.
Which way a given store errs depends on whether its trigger-maintained counter has drifted from
its reference rows — a property the preview depends on and the apply path does not.

The existing preview fixture makes the drift structural rather than accidental: the test
`garbage_collection_preview_reads_only_and_reports_bounds` in
`crates/eggwork-server/src/operations.rs` builds a synthetic blob database whose `blobs` table has
columns `(reference_count, size_bytes, last_used_unix_ms)` — **no `digest` column** — so the
canonical predicate cannot even be evaluated against it.

### 2.2 The path layout re-derived outside the store (WA-3)

`crates/eggwork-server/src/blob.rs` owns the shard layout in exactly one place:
`BlobStore::path_for` returns `root.join(&digest.as_str()[..2]).join(digest.as_str())`.
`architecture/data-plane.md` records that layout and the reason it is a pure function of the
digest: a digest that survives `BlobDigest::parse` cannot name a location outside the blob root.

`crates/eggwork-server/src/operations.rs` re-derives it by hand in the operator's blob inspection
verb (`inspect_blob`): it joins `config.blob_root` with `&digest.as_str()[..2]` for the shard
directory and then joins the full digest for the object path. `crates/eggwork-server/src/workspace.rs`
reaches the same place from the other direction, in `materialize_tree`, by calling
`blobs.path_for(digest)` and then opening the result with a plain `File::open`, accepting the bytes
on a **size-only** check (`io::copy` must yield exactly `size_bytes`).

The materializer's bypass is narrower than it looks but it is real. `materialize` does verify every
referenced blob in its pre-flight, through `blobs.stored_size(digest)` and then
`blobs.verify_existing(digest)`. But `materialize_tree` re-opens the object afterwards and copies
it without re-verifying, so the bytes a workspace actually receives are checked only by length, and
the check that did run happened before the copy.

### 2.3 The owner-id composite inlined at two sites (WA-4)

`crates/eggwork-server/src/workspace.rs` defines exactly one producer for the manifest reference
owner id:

```rust
fn manifest_owner_id(principal_id: &PrincipalId, manifest_digest: &str) -> String
```

`architecture/data-plane.md` records the `"{principal}:{manifest_digest}"` composite as part of the
reference-accounting contract for owner kind `"manifest"`.

Two call sites do not call it. In `WorkspaceManager::manifest_garbage_collect` and in
`WorkspaceManager::reconcile_manifests`, the `principal_id` is read out of a SQL row as a `String`,
and the composite is built inline as `format!("{principal_id}:{manifest_digest}")`.

**The cause is a signature mismatch, not carelessness.** The helper takes `&PrincipalId`, and
`PrincipalId::new` returns a `Result`, so at these sites there is no validated `PrincipalId` to
hand it. In `manifest_garbage_collect` the row's principal was written from a validated
`PrincipalId` and could simply be re-validated. In `reconcile_manifests` the inlined site is
specifically the branch reached when `PrincipalId::new` **failed** — a corrupt row — so no
validated principal exists there at all, and the composite must still be produced in order to
release the orphan reference. A plan that says "use the existing helper" without resolving this
mismatch is assuming the problem away.

### 2.4 The historical claim that is no longer sufficient

`plans/closure/workspace-artifact-transport/004-status.md` records that the collector ordering is
"in both `NodeServer::collect_garbage` and the operator `collect_garbage` path, with matching
dry-run previews". For the operator path's blob candidates, the code does not deliver that.
Per `plans/003-planning-process.md#corrective-work`, that closure record stays immutable; this
corrective owns the defect.

### 2.5 A note on scope

`NodeServer::collect_garbage` in `crates/eggwork-server/src/lib.rs` is documented as *"Run a
bounded local maintenance pass; no remote GC route is exposed."* Its only caller in the
repository is a test. The operator's `eggworkd gc` path and this in-process path are two routes to
the same three stores. The plan should note whether the two can be unified, or whether the
in-process path can be removed, as an **in-scope simplification opportunity** — but it must not
let that question swallow the milestone. The three findings above are the deliverable.

## 3. Scope

In scope:

- the candidate selection used by the operator's GC preview and by the operator's GC apply pass,
  for blobs, workspaces, retained manifests, and artifacts;
- the operator's blob inspection verb (`inspect_blob`) and the blob path layout it depends on;
- the workspace materializer's acquisition of blob bytes;
- the manifest reference owner-id composite and its two inlined call sites;
- the store metadata filenames that the operator re-derives.

Out of scope:

- the *content* of the blob, workspace, manifest, and artifact stores: no schema change, no row
  rewrite, no new column, no new table, no new index.
- the artifact GC / blob GC ordering requirement (architecture/data-plane.md already records that
  reclaiming an artifact's bytes needs artifact GC then blob GC, guaranteed by call order). This
  plan must preserve that ordering; it does not change it.
- divergence 18 of the `architecture/overview.md` table (artifact GC does not release blob
  references). That remains a recorded, accepted characteristic.
- the blob store's `reference_count` denormalization. The plan removes the preview's dependence on
  the counter; it does not remove the counter or migrate a store whose counter has drifted.
- the double hash in `crates/eggwork-server/src/artifact.rs`. It is documented and accepted.
- M005a (the declared-artifact capture fail-closed contract). The two milestones are independent
  and may run in either order.

## 4. Invariants and non-goals

### 4.1 The invariant this plan establishes

**A data-plane fact that a caller or an operator can observe has exactly one producer, and every
other place consumes it rather than restating it.**

Applied to this milestone, that means:

1. **Candidate selection has one owner.** The operator's dry-run report and the operator's apply
   pass evaluate the *same* store-owned statement. Divergence between them must be structurally
   impossible, not merely unlikely.
2. **The owner-module rule becomes enforced.** The statement recorded on the doc comment of
   `BlobStore::unreferenced_candidates` — that the same statement must be evaluated in both
   places — becomes a tested property rather than a comment. A copy that only a doc comment
   forbids is still a copy.
3. **The path layout has one owner.** No module outside `blob.rs` reconstructs the shard layout.
4. **The materializer verifies the bytes it writes.** A workspace's file contents are checked
   against the manifest digest, not only against a length.
5. **The owner-id composite has one producer.** No module formats it inline.

### 4.2 Non-goals

This plan does not:

- change any wire protocol, route, capability string, `NodeGcReport` field, or `ResourceInspection`
  field;
- change any stored format, schema, row, or on-disk file naming;
- change the GC *ordering* (workspaces → manifests → artifacts → blobs), the bounded-per-pass
  semantics, the `clamp(1, 1024)` limit, or the exclusivity of the apply pass;
- change the preview's non-mutating property. `docs/operations.md` states that `gc` "is a preview
  by default and reads only", and an existing test asserts it. The preview must remain
  non-mutating, and the apply pass must remain the only path that deletes;
- change the apply pass's requirement for an exclusive lock on node state;
- weaken authentication, authorization, lease renewal, generation fencing, or any digest bound;
- introduce a queue, a worker selection policy, a priority or fairness rule, a DAG, or a semantic
  retry. Eggwork remains a fixed-target, scheduler-free execution fabric;
- remove the `reference_count` column, the triggers that maintain it, or any other denormalization.

## 5. Required production changes

### 5.1 One candidate selection, used by both the preview and the apply pass

- The candidate selection for blobs, workspaces, retained manifests, and artifacts must live in
  the owner module (`blob.rs`, `workspace.rs`, `artifact.rs`) and must be the **only** place the
  predicate is written.
- The operator's preview must obtain its candidate sets through that owner code. It must not
  contain its own SQL.
- The apply path already calls the owner collectors. It must continue to.

**The trap the plan must call out.** The obvious implementation — "just open the store in the
preview and call its own dry-run collector" — is wrong as it stands, because `BlobStore::open` is
**not** read-only: it creates the root directory and the metadata database if they are absent, and
it calls `remove_stale_parts`, which deletes stale part files and reconciles index rows.
`WorkspaceManager::open` and `ArtifactStore::open` are likewise not read-only. The preview must
therefore reach the owner code through a **non-mutating** seam. The plan must choose and justify
one of:

- a read-only, non-reconciling open of each store that exposes the candidate query without
  mutating anything; or
- public, side-effect-free candidate-query entry points on the owner modules that the operator
  calls, implemented against the owner's own schema knowledge and its own statement; or
- a store-owned pure function that computes the candidate set from the store's metadata path
  without opening the store at all.

The plan must state which it takes and prove that the preview remains non-mutating — the existing
test `garbage_collection_preview_reads_only_and_reports_bounds` is the guard, and the corrected
version of it must keep that property while using the owner's predicate.

**The predicate must remain the reference-row predicate.** The plan must not reintroduce the
`blobs.reference_count` counter into any reclamation query. `architecture/data-plane.md` records
the deliberate reason.

### 5.2 The store metadata filenames have one owner

The operator must not spell `<blob_root>/metadata.sqlite`, `<workspace_root>/metadata.sqlite`, or
`<database_path>.artifacts.sqlite`. Each owner module must expose its metadata path (or the
operator must obtain it from the owner module), so that a change to a store's file name is a
one-line change in the owner.

### 5.3 The blob path layout has one owner

`inspect_blob` must resolve the shard directory and the object path through the store's path
ownership rather than re-deriving them.

The plan must handle the same non-mutating constraint as in 5.1: the inspection verb runs against
an offline store and must not create directories or reconcile metadata. The preferred shape is a
store-owned pure path function (root plus digest in, path out) that `BlobStore::path_for` itself
delegates to, so that the operator and the store share one implementation and the operator needs
no store instance. The plan must confirm that the inspection verb's existing safety properties are
preserved: it rejects a symlinked shard directory, rejects a symlinked object file, and cross-checks
the recorded size against the file's actual length.

### 5.4 The workspace materializer verifies the bytes it writes

`materialize_tree` must not accept blob bytes on a size-only check. The bytes written into the
workspace must be verified against the manifest digest.

- The verification must be performed by a store-owned routine, not by a re-derived hash in the
  materializer.
- The verification must be bound to the bytes that are actually copied, so that the pre-flight
  check and the copy cannot be separated by a change of the underlying file.
- The plan must record the performance consequence honestly: the pre-flight already re-hashes
  every referenced blob, and `architecture/data-plane.md` records that "blob download and workspace
  materialization both pay a full re-hash of every input". A verify-while-copying change keeps
  that cost roughly where it is; a verify-then-copy change adds one. The plan must state which it
  does.
- The plan must not widen any bound, and must not change the staging/rename protocol, the
  `0600`/`0700` permissions, the symlink refusal, or the staging cleanup behaviour.

### 5.5 The manifest owner-id composite has one producer

The `"{principal}:{manifest_digest}"` format must be written in exactly one place, and both
inlined call sites must go through it.

The plan must resolve the `&PrincipalId` versus `&str` mismatch explicitly, not assume it away.
The required shape is:

- one implementation of the format, reachable from both a validated-principal entry point and a
  raw-string entry point, with the validated entry point delegating to the raw one (or the
  equivalent) so that the format string itself exists once;
- the `manifest_garbage_collect` call site routed through it, with the row's principal validated
  where validation is possible;
- the `reconcile_manifests` call site routed through it, with the corrupt-row case handled
  deliberately — the plan must state what happens when the principal in a corrupt row cannot be
  parsed, and must confirm that the orphan reference is still released rather than leaked.

The plan must record *why* the raw form exists, so that a later reader does not "fix" it back
into an inline `format!` at the call site. It must also confirm that the composite's shape is
unchanged, because `architecture/data-plane.md` records that an owner id with no colon in it
cannot match any cache row and is therefore treated as an orphan.

### 5.6 The in-process collector path (bounded simplification, not the deliverable)

The plan must record a short, factual disposition of `NodeServer::collect_garbage`: whether it can
be unified with the operator path after this milestone, or whether it is retained as-is, and why.
It must not remove it, must not expose it as a remote route, and must not let this question delay
the three findings.

## 6. Failure, cancellation, restart, and contention semantics

- **Preview failure.** A preview that cannot evaluate the canonical statement must fail with a
  typed error, not degrade to zero candidates. A preview that reports zero because it could not
  read the store is indistinguishable from a preview that correctly found nothing, which is
  exactly the class of quiet wrongness this plan exists to remove.
- **Apply failure.** The apply path's existing semantics are unchanged: a failure inside the
  blob unlink loop still commits the work already done before surfacing the error, because
  rolling it back would resurrect rows for blobs whose files no longer exist. The plan must not
  change that.
- **Ordering.** Workspace GC → retained-manifest GC → artifact GC → blob GC must be preserved in
  both the preview and the apply pass, because the ordering is what makes a blob that has just
  become unreferenced collectable in the same pass.
- **Exclusivity.** The apply pass takes an exclusive lock on node state and therefore fails while
  the node is running. That is deliberate and unchanged. The preview must remain lock-free and
  non-mutating.
- **Restart.** Neither path may introduce a durable side effect that a restart could observe as a
  half-completed collection. The apply pass's existing per-store transaction boundaries are
  unchanged.
- **Contention.** The blob collector takes the blob store's write lock; the plan must not change
  that, and must not introduce a second collector that could run concurrently with it.
- **Cancellation.** The operator's `gc` verb has no cancellation semantics today and the plan must
  not add any.

## 7. Compatibility and migration

**This plan changes no public protocol surface and no stored format.** Specifically:

- no route, request shape, response shape, capability string, `NodeGcReport` field, or
  `ResourceInspection` field changes;
- no SQLite schema changes in the blob, workspace, artifact, or journal stores: no new table, no
  new column, no new index, no changed constraint;
- no on-disk file name changes: the blob shard layout, the store metadata file names, the artifact
  metadata file name, the workspace tree layout, and the `.part-` / `.staging-` transient names
  are all unchanged;
- no stored row is rewritten, and no migration runs. A store whose `blobs.reference_count` has
  drifted from its `blob_references` rows is not repaired by this plan; the plan simply stops the
  preview from depending on the drift;
- the `"{principal}:{manifest_digest}"` composite is unchanged, so existing manifest blob
  references remain releasable by owner id;
- the operator's `gc` output shape is unchanged. Only the *correctness* of the numbers it reports
  changes.

The one compatibility consequence the plan must record is **performance**: if the materializer
verifies bytes while copying, the materialization path performs a digest computation per file that
it does not perform today. `architecture/data-plane.md` already records that workspace
materialization pays a full re-hash of every input; the plan must state the resulting cost and
whether it changes.

## 8. Required tests

Every test must be non-vacuous, and the closure record must say how non-vacuity was established.

### 8.1 The preview/apply agreement test (required by the milestone)

A test that proves the preview and the apply pass agree, on the same store state and the same
clock.

- Build a **real** store through the owner module's own open, not a hand-built synthetic
  database. The existing preview fixture creates a `blobs` table with no `digest` column, which
  is a fixture shaped to the hand-copied predicate; it must be replaced.
- Seed blobs in each distinct state the predicates can disagree about: a blob with a live
  reference, a blob with only an expired reference, a blob with no reference rows at all, and — if
  the plan can construct it without fabricating state — a blob whose `reference_count` disagrees
  with its reference rows.
- Run the preview and record the candidate set it reports.
- Run the apply pass and record the set it actually removes.
- Assert **`preview ⊆ applied`**, and assert **equality for the same inputs** in the case where
  the store contains no expired references.

**The subset relation is the correct invariant, and the plan must say why.** The owner module's
doc comment records it: the preview selects *before* expired blob references are deleted and the
real pass selects *after*, so on a store that has expired references the apply pass may legitimately
collect strictly more than the preview predicted. A test that asserted bare equality would be
asserting the wrong property and would fail for a correct implementation. Where the inputs contain
no expired references, the two sets must be equal.

### 8.2 The single-predicate test

A test that proves the operator's preview does not contain its own candidate SQL: the preview's
candidate set for blobs must equal the set produced by the owner module's canonical statement
evaluated against the same store. This is what makes the owner-module doc comment an enforced
rule rather than a stated one.

A structural guard is acceptable in addition to the behavioural test — for example, a check that
the operator module contains no reclamation predicate of its own — provided the guard is precise
about what it forbids and does not produce false positives on the owner modules.

### 8.3 The blob inspection test

A test that proves `inspect_blob` resolves an object that exists in a real store, and that its
existing safety properties hold: a symlinked shard directory and a symlinked object file are
refused, and a recorded size that disagrees with the file's length is reported as a data error.
A companion test must prove that the inspection verb does not depend on the shard layout being
spelled the same way twice — concretely, that it still resolves the object after the layout is
expressed only through the owner.

### 8.4 The materializer verification test

A test that proves the workspace materializer refuses blob bytes that do not match the manifest
digest, and that it still materializes correct bytes. The negative case must be non-vacuous: a
blob whose *length* matches the manifest but whose *content* does not must be refused rather than
written into the tree.

### 8.5 The owner-id test

A test that proves the manifest reference owner id has one producer: that the composite released
by `manifest_garbage_collect` and by `reconcile_manifests` is the composite under which the
reference was created, including for a row whose principal does not parse. The negative case must
show that a reference is actually released, so that the test fails if the composite is wrong in a
way that orphans the pin.

### 8.6 The non-mutating preview test

The existing property — the preview reads only — must be retained by the corrected fixture, and
must be proven on a store that does not yet exist, so that the preview cannot silently acquire a
creating or reconciling open.

### 8.7 Regression guards

- The existing store-level GC tests, the workspace retention test, the artifact retention test, and
  the crash-reconciliation test must remain green and keep their original meaning.
- The `NodeServer::collect_garbage` integration test must remain green.
- The `eggworkd gc` command surface must be unchanged.

## 9. Required verification commands

The repository's pre-submit gate, in the repository's own spelling:

```text
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

Additional, for this milestone specifically:

- the new preview/apply agreement test, run on its own, and then the whole `eggwork-server` test
  set;
- a repeated run of the agreement test with the workspace's ordinary parallel test execution, to
  confirm the corrected fixture does not depend on serial ordering.

`--locked` is load-bearing on `clippy` and `test`: a version bump that skips `Cargo.lock` passes
both and then fails every release build.

## 10. Documentation updates

- `architecture/data-plane.md` — the blob-store section must describe the single candidate
  statement and the one-owner rule; the GC section must describe the preview/apply relationship
  accurately, including the subset relation; the materialization section must describe the
  verification bound to the copied bytes; and the "Honest gaps" list must be updated. Its stale
  line-number citations must be replaced with symbol references in the same pass.
- `architecture/operations-cli.md` — its description of `inspect_blob` currently describes the
  shard resolution in the operator's own terms; it must describe the store-owned resolution. Its
  description of the GC preview and apply must be updated to state the one-owner rule and the
  subset relation. Its stale line-number citations must be replaced with symbol references in the
  same pass.
- `docs/operations.md` — the GC section must remain accurate about the preview being read-only
  and the apply taking an exclusive lock. If the corrected preview's behaviour is observably
  different (it is not expected to be), that document must change.
- `README.md` — no change is expected. If the plan discovers one, it must be justified.
- `plans/closure/workspace-artifact-transport/005b-status.md` — the closure record itself.

## 11. Acceptance criteria

1. The operator's dry-run report and the operator's apply pass select candidates through the same
   store-owned statement; the operator module contains no candidate predicate of its own.
2. The blob candidate predicate is the reference-row predicate
   (`NOT EXISTS (SELECT 1 FROM blob_references ...)`), not the `reference_count` counter.
3. The rule recorded on the doc comment of `BlobStore::unreferenced_candidates` is enforced by a
   test rather than stated only in a comment.
4. The preview remains non-mutating, proven on a store that does not yet exist.
5. A test proves the preview's candidate set is a subset of the apply pass's removed set, and is
   equal to it for the same inputs when the store contains no expired references.
6. `inspect_blob` resolves object paths through the store's path ownership, and its symlink and
   size-cross-check properties are preserved.
7. The workspace materializer refuses blob content that does not match the manifest digest, even
   when the length matches, and the refusal is proven non-vacuous.
8. The manifest owner-id composite has exactly one producer; both inlined call sites use it; the
   `&PrincipalId` versus `&str` mismatch is resolved by an explicit decision; and a corrupt-row
   case still releases its reference.
9. No public protocol surface changed: no route, request, response, capability string,
   `NodeGcReport` field, or `ResourceInspection` field.
10. No stored format changed: no schema, row, file name, shard layout, or composite format.
11. The GC ordering (workspaces → manifests → artifacts → blobs), the bounded-per-pass semantics,
    the `clamp(1, 1024)` limit, and the apply pass's exclusive lock are unchanged.
12. No bound is widened; no authentication, lease, generation, fencing, or digest-bound semantics
    weakened; no queue, worker selection, priority, fairness, DAG, or semantic retry introduced.
13. `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` and
    `cargo test --locked --workspace --all-targets` are green on the same head.

## 12. Stop conditions

Stop and re-plan, rather than absorbing the work, if any of the following holds:

- making the preview reach the owner's canonical statement requires the preview to mutate the
  store, or requires the apply pass to stop taking an exclusive lock. Both would change an
  operator-visible safety property.
- unifying the preview and the apply pass turns out to require a change to the wire shape of
  `NodeGcReport`, or to the `gc` command surface. That is a protocol change and needs its own
  decision.
- the materializer's verified path cannot be implemented without a performance regression that the
  repository cannot accept, or without widening a bound.
- the owner-id resolution requires changing the composite format itself, which would orphan every
  existing manifest blob reference. That is a stored-format change and needs an ADR.
- the implementation discovers that the candidate-selection divergence has a second instance in a
  module this plan does not name. That is a new finding for the addendum, not for this milestone.
- the work turns out to require changing `architecture/` deep dives' semantics rather than
  describing them, or requires an ADR under
  `plans/003-planning-process.md#architecture-changes`.

## 13. Closure evidence required

Create:

- `plans/closure/workspace-artifact-transport/005b-status.md`.

The record must distinguish code inspection from executed evidence and must include:

- the reviewed head and the implementation commits;
- a requirement-to-evidence matrix, with one row per acceptance criterion in section 11;
- the exact commands run, from the list in section 9, with their results;
- the **non-mutating-preview evidence**: how the preview was proven not to mutate a store that did
  not previously exist;
- the **preview/apply agreement evidence**: the seeded store states, the candidate set the preview
  reported, the set the apply pass removed, and the relation observed between them, with the
  subset relation stated explicitly and the equality case demonstrated separately;
- the non-vacuity evidence for each new test, including how the digest-mismatch materializer case
  was shown to fail before the fix. For the corrupt-principal owner-id case, the pre-fix behaviour
  must be established empirically rather than assumed: if the pre-fix inline format produced the
  identical composite (the defect being duplication rather than a wrong released value), then the
  closure record states that plainly and cites the structural guard that enforces the single
  producer, instead of claiming a pre-fix failure that did not occur. A closure record must not
  assert a failing-before-the-fix result it did not observe;
- the **compatibility review**: an explicit statement that no wire shape and no stored format
  changed, with the performance consequence of the materializer change recorded;
- the documentation status for `architecture/data-plane.md` and `architecture/operations-cli.md`,
  including the replacement of their stale line-number citations with symbol references;
- the disposition of the in-process collector path recorded in section 5.6;
- unresolved findings by severity, with owners;
- the registry and roadmap disposition.

Do not claim a platform or feature is qualified merely because its code compiles.

## 14. Handoff notes

- **Start with the fixture.** The single most consequential edit is replacing the synthetic preview
  fixture — whose `blobs` table has no `digest` column — with a real store built by the owner
  module. That one change is what makes the canonical predicate testable at all.
- **The subset relation is not a weaker claim.** A test that asserts bare equality between the
  preview's set and the apply pass's set will fail for a correct implementation, because the two
  select at different points in the transaction. The owner module's doc comment records exactly
  this. Assert the subset relation, and assert equality separately for the no-expired-references
  case.
- **The preview must not gain a mutating open.** `BlobStore::open`, `WorkspaceManager::open`, and
  `ArtifactStore::open` all create and reconcile. A preview that opens a store has changed the
  operator's contract even if the visible numbers are the same.
- **Do not "just use the existing helper" for the owner id.** The helper takes `&PrincipalId`
  and the SQL row gives a `&str`; one of those call sites is reached precisely when the principal
  does not parse. The plan must resolve that, and must record why the resolved shape exists.
- **The materializer's cost is real.** Say plainly in the closure record whether the change adds a
  digest computation per materialized file or reuses the one the pre-flight already performs.
- **Boundary discipline.** The data plane owns the candidate predicate, the path layout, and the
  owner-id format. The operator consumes them. Do not move an operator concern into a store, and
  do not let the operator grow its own copy of a store's knowledge.
- **Do not touch M005a's findings.** The declared-artifact capture fail-closed contract belongs to
  M005a. The two milestones are independent and may run in either order.
- **Do not edit `plans/registry.md`** in this milestone; the registry is owned by the planning
  process, not by the implementing agent.
- **Do not edit `plans/000-` through `plans/003-`.** If the corrective appears to require a change
  to a canonical document, stop and escalate under
  `plans/003-planning-process.md#architecture-changes`.
