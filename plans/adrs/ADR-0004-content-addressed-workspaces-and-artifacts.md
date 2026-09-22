# ADR-0004: Content-Addressed Portable Workspaces and Declared Artifacts

Status: accepted

Date: 2026-09-22

Decision owners: project maintainers

Related specification:

- `plans/000-long-term-specification.md#11-workspace-and-artifact-model`
- `plans/001-terminology-and-domain-model.md#10-workspace`
- `plans/002-long-term-roadmap.md#phase-4--content-addressed-workspaces-and-artifacts`

## Context

Remote execution is only useful when the node can receive the inputs required by a command and return useful outputs. Directly passing host paths does not work across machines and would couple the protocol to one filesystem layout.

CodeGG adds another constraint: it already owns repository/worktree identity, leases, dirty/conflicted state, and Git integration. Eggwork must not absorb that domain merely to transport a workspace.

Build systems demonstrate the value of content-addressed inputs, but Eggwork is not a build cache and does not need a complete Bazel Remote Execution implementation.

## Decision drivers

- portable across hosts and operating systems;
- deduplicate unchanged file content;
- preserve integrity;
- avoid Git as a core dependency;
- safe path materialization;
- keep large content out of control-plane JSON;
- support CodeGG without stealing worktree/Git ownership;
- permit later optimized materializers.

## Considered options

### Option A — Remote absolute paths / pre-provisioned paths only

Simple but not portable and difficult to use with CodeGG-selected worktrees.

Rejected as the primary contract. A pre-existing node-local workspace may be an explicitly configured advanced source later.

### Option B — Git repository URL + commit as the canonical workspace

Efficient for clean repositories but cannot represent arbitrary uncommitted files, generated inputs, non-Git projects, or restricted networks without extra mechanisms.

Rejected as the core model.

### Option C — Content-addressed manifest + blob store, with optional future Git adapters

Selected.

## Decision

1. The portable core input is a bounded WorkspaceManifest.
2. File bytes are addressed by cryptographic BlobDigest; SHA-256 is the required initial digest.
3. Control objects contain digests/metadata, not large inline file bodies.
4. The client may ask the node which digests are missing before upload.
5. Blob upload/download is streamed and digest-verified.
6. Workspace paths are relative and normalized. Absolute paths, parent traversal, device nodes, and unsupported special-file types are rejected.
7. Symlink handling is explicit and must not permit materialization escape.
8. Materialization occurs under node-owned roots with execution/workspace ownership recorded.
9. A workspace manifest does not imply Git history, branch identity, or merge semantics.
10. Outputs are declared. Eggwork captures only declared paths/patterns by default and returns Artifact records/digests.
11. Artifact retrieval uses handles/digests and streaming bodies.
12. Storage has explicit quotas and garbage-collection ownership.
13. Git-aware transfer may later optimize construction from commits/bundles/patches, but must project into or remain compatible with the neutral workspace contract.
14. CodeGG remains authoritative for its repository/worktree state and validates any returned Git evidence before integration.

## Consequences

### Positive

- general-purpose non-Git execution;
- safe deduplication;
- integrity checking;
- portable CodeGG snapshots;
- future CAS/delta optimization without changing execution semantics.

### Negative

- initial implementation must build a manifest/blob store;
- naïve first snapshots may be slower than Git-aware transfer;
- filesystem metadata portability needs deliberate scope.

## Initial metadata scope

The first stable manifest should support only metadata required for common developer execution, such as regular files, directories, executable bit where portable, and a deliberately reviewed symlink representation.

Ownership IDs, ACLs, xattrs, device nodes, sockets, FIFOs, and host-specific metadata SHOULD remain unsupported until explicitly designed.

## Verification

Required tests include:

- digest mismatch;
- duplicate blob upload;
- missing blob detection;
- traversal and absolute path rejection;
- symlink escape;
- conflicting file/directory entries;
- case/canonicalization collision where relevant;
- atomic/cleanup behavior after failed materialization;
- declared artifact capture;
- undeclared output exclusion;
- large blob streaming without control-plane buffering;
- quota exhaustion with typed failure.

## Supersession

None.
