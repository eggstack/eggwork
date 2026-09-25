# Workspace and Artifact Transport Roadmap

Status: active roadmap; M001-M003 closed, M004 ready

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

Status: ready

Implementation plan:

- `plans/implementation/workspace-artifact-transport/004-reusable-manifest-cas-and-derived-materialization.md`

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
