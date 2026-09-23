# Workspace and Artifact Transport M001 — Blob Store and Digest Protocol

Status: active

Dependency evidence: `plans/closure/control-plane-protocol/002-status.md`

Source roadmap:

- `plans/subsystems/workspace-artifact-transport-roadmap.md`

Canonical/ADR references:

- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`
- `plans/000-long-term-specification.md#11-workspace-and-artifact-model`

## 1. Objective

Implement the integrity-checked content-addressed byte store and fixed-target transfer protocol used by later workspace materialization and artifact retrieval.

## 2. Scope

In:

- SHA-256 BlobDigest;
- node-owned blob storage root;
- find-missing operation;
- streamed upload;
- streamed authorized download;
- atomic finalize after digest verification;
- duplicate/concurrent upload handling;
- size/quota limits;
- temporary-part cleanup;
- reference/last-used metadata sufficient for later GC.

Out:

- workspace materialization;
- Git;
- artifacts as semantic outputs;
- chunked/delta transfer;
- shared external CAS.

## 3. Storage rules

- request never chooses physical storage path;
- digest maps through a fixed safe layout;
- upload writes to owner-private temporary storage;
- hash while streaming;
- enforce declared and hard maximum length;
- finalize only after digest/length verification;
- duplicate valid blob is success/no-op;
- corrupted existing blob is quarantined/fails loudly according to defined policy;
- fsync/durability policy must be documented rather than assumed.

## 4. Protocol rules

Control JSON carries digest/size metadata.

Blob bytes stream in request/response body.

Find-missing is bounded by number of digests and response size.

Authorization checks apply to upload/read operations according to the Security M001 policy once available; until then the control-plane authenticated principal context must still be threaded, not ignored.

## 5. Failure and contention

Define typed outcomes for:

- invalid digest;
- too large;
- quota exceeded;
- length mismatch;
- digest mismatch;
- storage IO;
- concurrent same-digest upload;
- partial/disconnected upload;
- authorization denial.

No failed upload may create a readable valid blob entry.

## 6. Tests

- known digest vector;
- empty blob;
- large streaming blob;
- wrong digest;
- wrong declared length;
- connection drop;
- duplicate sequential/concurrent upload;
- quota edge;
- corrupt on-disk fixture;
- path injection impossible through digest parser;
- unauthorized read/write;
- restart cleanup of stale partials.

## 7. Acceptance criteria

1. Blob identity is content-derived and verified.
2. Large blobs are never buffered wholly by the control plane.
3. Duplicate data deduplicates safely.
4. Failed transfer cannot masquerade as complete.
5. Physical paths are node-owned.
6. Quota behavior is deterministic and bounded.

## 8. Stop conditions

Stop if implementation requires trusting client-provided filesystem paths or weakening digest verification for streaming convenience.

## 9. Closure evidence

Create `plans/closure/workspace-artifact-transport/001-status.md` with transfer, corruption, concurrency, quota, and cleanup evidence.
