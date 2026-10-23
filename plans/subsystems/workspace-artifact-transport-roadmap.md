# Workspace and Artifact Transport Roadmap

Status: active roadmap; M001-M004 closed; M005a data-plane corrective implemented; M005b data-plane corrective implemented

Canonical authority:

- `plans/000-long-term-specification.md#11-workspace-and-artifact-model`
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`

## 1. Ownership boundary

This subsystem owns:

- BlobDigest and blob storage implementation;
- missing-blob discovery;
- streamed upload/download;
- WorkspaceManifest validation;
- safe materialization;
- execution-private workspace lifecycle;
- declared output collection;
- Artifact records/retrieval;
- storage quotas, leases/references, and garbage collection.

It does not own Git repository/worktree semantics, CodeGG branch integration, scheduler placement, or artifact interpretation by downstream applications.

## 2. Durable invariants

1. Blob bytes must match their digest.
2. Workspace manifest paths are relative, normalized, and confined.
3. Materialization cannot escape the node-owned root.
4. Unsupported special-file metadata is rejected rather than guessed.
5. Large bodies stream; they are not base64-expanded into control JSON.
6. Declared outputs are the default artifact capture boundary.
7. Storage exhaustion produces typed failure and bounded cleanup.
8. Git-aware optimization cannot redefine the neutral workspace contract.

## 3. Milestones

### M001 — Blob store and digest protocol

Class: infrastructure/capability

Status: closed

Implementation plan:

- `plans/implementation/workspace-artifact-transport/001-blob-store-and-digest-protocol.md`

Objective:

Implement SHA-256 blob identity, find-missing, streamed verified upload/download, authorization hooks, storage layout, quotas, and deterministic corruption handling.

Exit conditions:

- digest mismatch rejected;
- duplicate content deduplicated;
- large transfer remains streaming;
- partial upload cleanup bounded;
- quota exhaustion typed;
- blob paths cannot be chosen by request text.

### M002 — Workspace manifest and safe materialization

Class: capability/invariant

Status: closed

Implementation plan:

- `plans/implementation/workspace-artifact-transport/002-workspace-manifest-and-safe-materialization.md`

Objective:

Materialize a bounded portable tree into a node-owned workspace without traversal/symlink/special-file escape.

Exit conditions:

- regular files/directories/executable metadata work;
- path and case/canonicalization conflicts are caught;
- symlink policy cannot escape;
- missing blobs fail before execution;
- failed materialization leaves no ambiguous ready workspace.

### M003 — Declared artifacts, retention, and GC

Class: capability/infrastructure

Status: closed

Implementation plan:

- `plans/implementation/workspace-artifact-transport/003-declared-artifacts-retention-and-gc.md`

Objective:

Collect declared execution outputs, publish artifact metadata/digests, stream retrieval, and safely reclaim unreferenced blob/workspace/artifact state.

Exit conditions:

- only declared outputs captured by default;
- artifact digest verified;
- large artifact stays out of terminal control object;
- terminal execution retains referenced artifacts for configured period;
- GC never removes live referenced content;
- quota/GC contention is deterministic.

### M004 — Reusable manifest CAS and derived materialization

Class: infrastructure/capability

Status: closed

Implementation plan:

- `plans/implementation/workspace-artifact-transport/004-reusable-manifest-cas-and-derived-materialization.md`

Closure record:

- `plans/closure/workspace-artifact-transport/004-status.md`

Objective:

Persist validated canonical manifests in a principal-scoped bounded cache and add a versioned derived-workspace route that applies a deterministic patch to a retained base manifest before using the existing safe materializer.

Exit conditions:

- existing full-manifest workspace creation remains wire-compatible;
- `workspace.derive.v1` is separately advertised;
- same-principal derived materialization produces the exact canonical digest of an equivalent full manifest;
- missing/expired bases are typed, non-destructive misses;
- retained manifests pin required blobs only for bounded retention;
- cross-principal manifest reuse is impossible;
- no Git semantics or shared writable workspace tree enters Eggwork.

### M005a — Declared-artifact capture fail-closed corrective

Class: invariant

Status: implemented

Corrective addendum:

- `plans/subsystems/workspace-artifact-transport-data-plane-corrective-addendum.md`

Implementation plan:

- `plans/implementation/workspace-artifact-transport/005a-declared-artifact-capture-fail-closed-corrective.md`

Closure record:

- `plans/closure/workspace-artifact-transport/005a-status.md`

Objective:

Stop a shipped target from silently losing declared outputs. On a non-Unix target the safe artifact-capture open is refused, and the execution still reports `Succeeded` with a zero artifact count and a nested finalization-failure marker. An execution whose declared outputs could not be captured must not present as a successful execution with missing artifacts.

Exit conditions:

- a failed capture is visible in the terminal state, the event stream, and to the caller, not only as a nested marker;
- the caller can distinguish "no declared outputs" from "declared outputs that were not captured";
- the platform claim in the README and in the divergences table matches the code;
- the required-isolation and fail-closed rules are unchanged.

Implemented:

- the terminal result carries a first-class capture disposition, `ExecutionResult.artifact_capture` (`eggwork_core::ArtifactCaptureDisposition`), so the caller reads "did the declared outputs arrive" from the result instead of comparing an artifact count against a declared-output count it does not have;
- the mapping from a capture attempt to the terminal facts is one decision, `eggwork_server::decide_artifact_capture`, reachable from outside the crate: a declared output that was not captured never leaves the execution `Succeeded`, and the target's own failure is never overwritten by the node's;
- the platform's capture capability is one fact, `artifact::capture_capability`, stated next to the platform-specific `open_file_beneath` that can refuse a capture, and re-exported from the crate root as `artifact_capture_capability`;
- the `artifact.declared.v1` capability name is unchanged and its advertisement follows the fact: a node whose platform cannot capture does not advertise it;
- the capture failure is published on the server's existing event-publishing path as a diagnostic of class `artifact.capture.failed`, with the grammar, the bound, and the truncation rule in `architecture/protocol.md`;
- the capture-eligibility rule is unchanged: a cancelled, timed-out, interrupted, or output-limit-killed execution still captures nothing, and it reports its own terminal state;
- the terminal snapshot is not byte-identical across the change: the result gained a field, and a capture that did not complete no longer reports `Succeeded`.

### M005b — GC preview and store ownership corrective

Class: invariant

Status: implemented

Corrective addendum:

- `plans/subsystems/workspace-artifact-transport-data-plane-corrective-addendum.md`

Implementation plan:

- `plans/implementation/workspace-artifact-transport/005b-gc-preview-and-store-ownership-corrective.md`

Closure record:

- `plans/closure/workspace-artifact-transport/005b-status.md`

Objective:

Make the operator's GC preview and the apply pass share one implementation, so they cannot disagree, and bring the blob store's path layout and the manifest owner-id format back under the modules that own them. The owner module already states that the dry run and the real run must share the candidate statement; that rule is currently violated in the sibling module, where the preview uses a different predicate from the one the apply pass uses.

Exit conditions:

- the dry-run report and the apply pass select candidates through the same store-owned statement;
- the operator's blob inspection goes through the store's path and ownership API rather than re-deriving the shard layout;
- the workspace materializer uses the store's verified-copy path, so the verification is bound to the bytes actually copied;
- the manifest owner-id composite has exactly one producer, and the two inlined call sites use it;
- a test proves that the preview and the apply pass agree;
- no public protocol surface and no stored format changes.

Implemented:

- the candidate predicate for blobs, workspaces, retained manifests, and artifacts is written once, in the owner module, and the operator's preview obtains its candidate sets through that owner code and holds no SQL of its own;
- the preview reaches the owner through a non-mutating seam - `blob::preview_gc`, `workspace::preview_gc`, and `artifact::preview_gc` open the metadata database read-only, because every owner `open` creates and reconciles;
- the blob path layout has one owner: `blob::object_path` and `blob::shard_path`, which `BlobStore::path_for` delegates to;
- the materializer verifies the bytes it writes while copying them, through the store's `copy_verified`, so the verification is bound to the bytes actually copied rather than to a separate earlier read;
- the manifest owner id has one producer, `manifest_owner_id_for_key`, with `manifest_owner_id` as the validated-principal entry point and the two inlined call sites routed through it.

## 4. Future optimized materializers

After M004, optional adapters may add:

- filesystem clone/reflink acceleration where safely qualified;
- chunk transport below the current blob object size;
- pre-existing trusted node workspace;
- shared external CAS;
- caller-side Git-aware planning that still resolves to Eggwork's Git-neutral manifest/patch contract.

Eggwork itself continues not to own Git commit/bundle/object semantics.

All such adapters must preserve path confinement, digest truthfulness, and caller-owned application semantics.

## 5. Verification strategy

- traversal corpus;
- symlink/junction escape tests;
- corrupted and truncated uploads;
- concurrent duplicate uploads;
- crash/cleanup boundary tests;
- filesystem case-sensitivity behavior;
- executable bit portability;
- undeclared output exclusion;
- artifact retrieval authorization;
- GC live-reference races.
