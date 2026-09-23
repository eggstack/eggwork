# Workspace and Artifact Transport M003 — Declared Artifacts, Retention, and GC

Status: closed

Dependency evidence: `plans/closure/workspace-artifact-transport/002-status.md`

Closure evidence: `plans/closure/workspace-artifact-transport/003-status.md`

Source roadmap:

- `plans/subsystems/workspace-artifact-transport-roadmap.md`

## 1. Objective

Capture bounded declared outputs from completed execution, expose integrity-checked artifact records/retrieval, and safely reclaim unreferenced workspace/blob/artifact data.

## 2. Declared outputs

ExecutionSpec may declare bounded output paths/patterns under the workspace.

Define:

- exact file;
- bounded directory tree if needed;
- optional glob only if semantics can be cross-platform and bounded.

Never default to uploading the entire workspace.

Artifact capture must apply the same confinement rules as input materialization.

## 3. Artifact model

Artifact metadata includes:

- ArtifactId;
- execution ID/generation;
- logical relative path;
- type;
- digest;
- size;
- executable/basic metadata as supported;
- creation timestamp;
- retention/reference state.

Large bytes remain in blob/content store.

## 4. Terminal ordering

Define explicit ordering between process terminal, artifact capture, cleanup, and final ExecutionResult.

A process may exit successfully while required artifact capture fails; terminal result must preserve both process outcome and artifact/finalization failure rather than silently calling it success.

Cancellation/timeout artifact policy must be explicit: capture only declared partial outputs if configured and safe, otherwise omit.

## 5. Retention and GC

Implement:

- execution/workspace/artifact reference ownership;
- terminal retention policy;
- unreferenced blob GC;
- stale partial cleanup;
- dry-run inspection;
- no deletion of live execution inputs/outputs;
- bounded GC batch/work;
- concurrency locking/generation where needed.

GC is maintenance, not a scheduler.

## 6. Tests

- declared file;
- nested declared directory if supported;
- undeclared output excluded;
- artifact digest;
- disappearing file during capture;
- symlink output escape;
- huge output/quota;
- cancel/timeout policy;
- GC while readers/execution live;
- duplicate blob referenced by two artifacts;
- terminal retention expiry;
- restart mid-GC.

## 7. Acceptance criteria

1. Artifact capture is explicit and confined.
2. Process and finalization outcomes remain distinguishable.
3. Artifact retrieval is streamed/authorized.
4. Live references cannot be collected.
5. GC work is bounded and inspectable.
6. Terminal result stays compact.

## 8. Closure evidence

Create `plans/closure/workspace-artifact-transport/003-status.md` with artifact/finalization matrix and live-reference GC race evidence.
