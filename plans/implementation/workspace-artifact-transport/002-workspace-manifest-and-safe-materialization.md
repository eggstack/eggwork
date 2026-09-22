# Workspace and Artifact Transport M002 — Workspace Manifest and Safe Materialization

Status: blocked on Workspace/Artifact M001 closure

Source roadmap:

- `plans/subsystems/workspace-artifact-transport-roadmap.md`

## 1. Objective

Materialize a portable bounded workspace tree from verified blobs into an execution-owned node root without path, symlink, special-file, or metadata escape.

## 2. Manifest v0 scope

Support only deliberately portable entries:

- directory;
- regular file + digest;
- executable bit where meaningful;
- symlink only if a safe reviewed policy can be implemented without escape.

Do not support devices, sockets, FIFOs, arbitrary ACL/xattr/owner metadata, or host absolute links.

Every path is relative UTF-8 or a deliberately documented byte/path representation. Pick one contract and test cross-platform behavior.

## 3. Validation before mutation

Validate complete manifest before materialization:

- entry count;
- total path bytes;
- depth;
- duplicate path;
- parent-child type conflict;
- absolute/root/prefix paths;
- `..` traversal;
- NUL/platform-invalid names;
- case/canonicalization collisions where applicable;
- symlink target policy;
- referenced digest presence;
- declared total logical bytes versus quotas.

Prefer a normalized internal tree over executing entries one by one during validation.

## 4. Materialization

- allocate under node-owned workspace root;
- associate WorkspaceId and owning ExecutionId/generation;
- stage into non-ready location;
- use safe descriptor/handle-relative operations where practical;
- never follow an attacker-controlled link during creation;
- atomically mark ready only after completion;
- failed preparation cleans safely or leaves explicit quarantined state;
- permissions are least privilege.

## 5. Execution integration

Control Plane accepts a workspace reference only when ready and authorized.

Runner receives an already-resolved local root and relative cwd; it does not resolve remote manifest semantics itself.

Workspace cleanup waits until execution/reader ownership ends.

## 6. Tests

- nested tree;
- executable fixture;
- missing digest;
- duplicate/conflicting paths;
- absolute/traversal variants;
- symlink escape matrix;
- case-collision platforms;
- over-depth/count/size;
- interrupted materialization;
- concurrent materialization of same manifest;
- cleanup with running execution;
- malicious pre-existing filesystem state.

## 7. Acceptance criteria

1. Valid manifest yields exactly the intended tree.
2. Invalid manifest mutates nothing visible as ready.
3. No entry escapes workspace root.
4. Missing/corrupt blobs prevent ready state.
5. Runner sees a stable authorized local root.
6. Workspace ownership/cleanup is deterministic.

## 8. Stop conditions

Stop if path safety depends on string prefix comparison alone, or if symlink support cannot be made fail-closed on a platform. Symlinks may remain unsupported initially.

## 9. Closure evidence

Create `plans/closure/workspace-artifact-transport/002-status.md` with adversarial path corpus and actual platform filesystem evidence.
