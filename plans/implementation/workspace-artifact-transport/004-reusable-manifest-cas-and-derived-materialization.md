# Workspace/Artifact M004 — Reusable Manifest CAS and Derived Materialization

Status: ready for handoff

Source roadmap:

- `plans/subsystems/workspace-artifact-transport-roadmap.md`

Canonical authority:

- `plans/000-long-term-specification.md#11-workspace-and-artifact-model`
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`
- `plans/003-planning-process.md`

Plan-authoring baseline:

- Eggwork `2566e6a54451468011119844824b644aea891b0c`

Downstream consumer:

- CodeGG Eggwork remote-execution M003 workspace-transfer optimization.

Primary class: infrastructure/capability

## 1. Objective

Add a neutral content-aware materialization primitive that lets a caller create a new execution-private workspace from a previously retained canonical workspace manifest plus a bounded deterministic patch.

The primitive reduces repeated full-manifest transfer while preserving the existing SHA-256 blob CAS, path-confinement rules, execution-private workspace ownership, and caller-owned application/Git semantics.

Eggwork MUST NOT learn Git commits, branches, worktrees, dirty-state semantics, or repository policy.

## 2. Current state and gap

The current workspace path already provides:

- canonical `WorkspaceManifest` validation and order-independent digest;
- SHA-256 blob CAS;
- `find_missing`/streamed blob upload, so unchanged blob bytes are already deduplicated;
- execution-private materialization;
- principal/workspace authorization;
- manifest digest returned in `WorkspaceReadyResponse`;
- live/terminal blob references and bounded GC.

The remaining repeated-transfer gap is:

1. each `POST /v1/workspaces` request carries the entire manifest;
2. workspace metadata persists only the canonical manifest digest, not the canonical manifest body;
3. a later workspace cannot name that manifest as a content base;
4. a downstream caller must therefore resend the whole manifest even when only a small set of paths changed.

This milestone optimizes that gap only. It is not a Git protocol and does not change blob-upload semantics.

## 3. Wire/API compatibility strategy

Do not break existing `POST /v1/workspaces` schema-v1 clients.

Add a separate versioned capability and route:

```text
workspace.derive.v1
POST /v1/workspaces/derive
```

Existing full-manifest creation remains valid and authoritative.

Add public client support equivalent to:

```rust
NodeClient::create_workspace_derived(
    workspace_id,
    handle,
    base_manifest_digest,
    patch,
)
```

The exact Rust type names may differ, but the operation must remain distinct from the existing full-manifest method so older callers require no wire migration.

## 4. Canonical patch model

Add protocol-neutral core types for a bounded manifest patch.

Recommended shape:

```rust
struct WorkspaceManifestPatch {
    schema_version: u16,
    base_manifest_digest: BlobDigest,
    entries: Vec<WorkspacePatchEntry>,
}

enum WorkspacePatchEntry {
    Remove { path: RelativePath },
    Directory { path: RelativePath },
    File {
        path: RelativePath,
        digest: BlobDigest,
        size_bytes: u64,
        executable: bool,
    },
}
```

Required semantics:

- schema version starts at 1;
- patch paths are unique;
- patch entry count is bounded by `MAX_WORKSPACE_ENTRIES` or a stricter explicit patch bound;
- `Remove` removes exactly one path;
- directory subtree removal requires explicit descendant removals; no implicit recursive delete semantics;
- directory/file variants are upserts at that exact path;
- rename is represented as remove + upsert;
- symlinks/special files remain unsupported;
- patch ordering does not affect the final canonical manifest;
- final result MUST pass the existing `WorkspaceManifest::validate` unchanged;
- final canonical digest MUST be computed by existing `WorkspaceManifest::digest`.

Do not create a second path validator.

## 5. Principal-scoped manifest store

Persist canonical manifest bodies by:

```text
(principal_id, manifest_digest)
```

not by digest alone.

Rationale:

- content reuse for one authenticated controller/principal is useful;
- manifest existence/path metadata must not become a new cross-principal information channel;
- the current authorization boundary remains transport-derived.

The manifest store must contain only validated canonical manifests.

A full workspace create that succeeds MUST register/refresh its manifest in this store.

A derived create MUST resolve its base manifest only within the authenticated principal's manifest scope.

Do not permit request payloads to choose or override the authoritative principal.

## 6. Retention and blob references

A retained manifest is useful only while its referenced blobs remain available.

Integrate manifest retention with the existing blob-reference system rather than creating another blob ownership mechanism.

Required behavior:

- retained manifests hold bounded blob references using a distinct owner kind;
- live workspaces using the manifest keep those references live;
- after terminalization, manifest/blob retention may follow the existing workspace/artifact retention horizon;
- multiple workspaces referencing the same principal+manifest extend retention monotonically as needed;
- manifest expiry releases its blob references;
- GC is bounded;
- a manifest record is never considered usable after its required blob references are unavailable;
- crash/restart reconciliation cannot leave permanent unbounded references.

No new global indefinite cache is allowed.

If implementation requires a small manifest metadata table/index, it belongs to the WorkspaceManager/storage subsystem, not to execution-store scheduling state.

## 7. Derived materialization flow

For `POST /v1/workspaces/derive`:

1. authenticate and authorize `WorkspaceCreate` for the target workspace;
2. validate request schema and handle/generation;
3. look up the principal-scoped base manifest;
4. if absent/expired, return typed `409 base_manifest_missing` with zero ready-workspace side effect;
5. apply the patch in a deterministic path map;
6. validate the final full manifest with the existing validator;
7. compute its canonical digest;
8. verify all referenced blobs are present/valid;
9. retain the final manifest/blob references;
10. materialize through the existing WorkspaceManager path;
11. return the same ownership/digest/logical-byte facts as normal workspace creation.

The final workspace remains a fresh execution-private tree. It is not an alias to or writable reuse of another execution's workspace directory.

## 8. Idempotency and identity

The current workspace identity/owner fencing remains authoritative.

Required cases:

- same workspace id + same owner + same final manifest -> idempotent ready result;
- same workspace id with different owner/generation/final manifest -> `workspace_identity_conflict`;
- repeating the same derived request after transport loss produces the same final manifest/workspace result;
- full then derived requests may converge on the same manifest digest without sharing execution ownership.

The optimization must not alter execution request-digest or generation/lease semantics.

## 9. Failure and fallback contract

The server never silently falls back from a derived request to a different base or full manifest.

Typed outcomes:

- missing/expired base -> `base_manifest_missing`;
- malformed patch/final tree -> `invalid_manifest` or a specific bounded patch error;
- missing/corrupt final blob -> existing missing/corrupt blob error;
- quota -> existing typed quota behavior;
- owner conflict -> existing workspace identity conflict.

A client MAY respond to `base_manifest_missing` by issuing the existing full-manifest create request. That choice belongs to the caller.

Do not use authorization/storage/internal errors as signals for fallback.

## 10. Materialization implementation

M004 does not require filesystem reflink/hardlink/template cloning.

The existing safe materializer remains authoritative and may copy files from the blob CAS exactly as today.

Hard links to writable shared content are explicitly out of scope because they could couple execution-private mutation.

Filesystem-level clone/reflink optimization may be a later implementation only if correctness and portability are separately qualified.

## 11. Capabilities and diagnostics

Advertise `workspace.derive.v1` only when the route/store is available.

Existing capabilities remain unchanged.

Add bounded operational counters where useful, for example:

- derived requests;
- derived hits;
- base-missing misses;
- full manifest registrations;
- patch entries applied.

Do not expose manifest path bodies in ordinary metrics/status output.

## 12. Security review

Required threat cases:

- principal B cannot derive from principal A's retained manifest;
- forged payload principal is ignored/rejected as today;
- patch traversal/case collision/device-name/symlink attempts fail through existing validation;
- remove/upsert combinations cannot create a tree the full-manifest path would reject;
- expired base cannot resurrect expired blob authority;
- missing-base response does not reveal another principal's manifest existence;
- derived workspace cannot mutate its base workspace or another execution workspace;
- blob digest truthfulness remains SHA-256 verified.

## 13. Required tests

Core:

- patch schema/duplicate-path/bounds;
- order independence;
- deterministic full-result digest;
- remove + upsert semantics;
- invalid final parent/path/case/symlink behavior.

Workspace/store:

- full create registers canonical manifest;
- same-principal derived hit;
- different-principal lookup behaves as missing;
- expired base behaves as missing;
- retained manifest pins referenced blobs;
- manifest GC releases blob references;
- crash/reopen reconciles bounded metadata;
- concurrent same-derived materialization is idempotent;
- same workspace id/different final digest conflicts.

Server/client:

- authenticated full -> derived flow;
- `workspace.derive.v1` advertised;
- typed `base_manifest_missing`;
- transport retry;
- authorization denial;
- request-body bound;
- final response digest equals locally reconstructed full manifest.

Regression:

- existing full create path unchanged;
- existing blob/workspace/artifact GC tests;
- ownership guard;
- restricted Landlock execution remains unaffected.

## 14. Verification

At minimum:

```bash
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
git diff --check
```

Add focused client/server loopback tests for the new route.

## 15. Acceptance criteria

1. Existing full-manifest wire contract remains compatible.
2. A successful full create makes its canonical manifest available as a same-principal derived base.
3. A bounded patch produces exactly the same canonical final manifest/digest as an equivalent full manifest.
4. Derived creation returns a fresh execution-private workspace.
5. Cross-principal manifest reuse is impossible.
6. Base retention pins required blobs and GC releases them boundedly.
7. Missing/expired base is a typed non-destructive miss.
8. Derived retry is idempotent.
9. Existing path/blob/workspace security invariants remain green.
10. No Git semantics enter Eggwork.
11. No hardlink/shared-writable workspace optimization is introduced.
12. No unresolved high/medium finding remains.

## 16. Stop conditions

Stop and record a blocker if:

- safe manifest reuse requires sharing a writable workspace tree;
- the derived path cannot reuse the exact full-manifest validator;
- retention cannot integrate with existing blob references without an unbounded second ownership system;
- principal scoping cannot be enforced before base lookup;
- compatibility would require changing schema-v1 `POST /v1/workspaces` semantics;
- implementation needs Git-specific types or repository operations;
- a caller must trust server-generated content not reconstructable into the canonical final manifest.

## 17. Closure evidence

Create:

- `plans/closure/workspace-artifact-transport/004-status.md`

Record:

- implementation SHA(s);
- exact new wire feature/route;
- manifest-store schema and principal scope;
- patch canonicalization/digest equivalence evidence;
- retention/GC evidence;
- cross-principal negative evidence;
- retry/conflict behavior;
- loopback client/server evidence;
- full regression/verification;
- residual findings;
- downstream CodeGG M003 disposition.
