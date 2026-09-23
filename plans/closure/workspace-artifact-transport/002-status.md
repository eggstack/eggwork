# Workspace and Artifact Transport M002 Closure — Manifest and Safe Materialization

Source plan: `plans/implementation/workspace-artifact-transport/002-workspace-manifest-and-safe-materialization.md`  
Subsystem roadmap: `plans/subsystems/workspace-artifact-transport-roadmap.md`  
Reviewed implementation commit: `428e2ce`  
Planning/closure commit: `03da005`

## Finding

M002 is closed for the exercised Linux host. Eggwork validates bounded portable workspace manifests, verifies referenced blobs, materializes files into a node-owned execution/generation-scoped root, and makes a workspace resolvable only after the staged tree is complete and durable. Symlinks are rejected in this manifest version. The runner accepts only the resolved local root; workspace ownership is checked against authenticated principal, execution ID, generation, and ready state.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Bounded, portable manifest contract | Core validation enforces entry count, depth, path bytes, logical bytes, normalized relative ASCII paths, explicit directory parents, duplicate/type conflicts, Windows reserved/illegal names, and case-fold collisions. Symlink entries are rejected. Tests cover traversal, conflicts, symlinks, depth, count, size, and order-independent manifest digest. |
| Blob identity and size checked before materialization | Workspace creation validates the complete manifest and verifies each referenced blob's indexed size and full content digest before staging. Missing blobs never produce a ready workspace. The integration path uploads a blob, creates a workspace and executes a command that reads the materialized file. |
| Confined staging and ready transition | Materialization uses a random storage key and newly created staging directory beneath the configured workspace root. Validated paths are joined only after manifest validation; files use exclusive creation, symlinks are not created, staging is synced and atomically renamed before durable Ready state. Startup reconciliation removes stale staging/untracked paths and detects missing roots. |
| Least-privilege permissions and ownership | On the exercised Unix/Linux host the root/directories are mode `0700`, regular files `0600`, and executable files `0700`. Resolve fences workspace access to principal, execution ID, generation, and Ready state. Integration verifies a different execution identity receives 409. |
| Quota, idempotency, and contention | Logical workspace quota is checked before materialization. Same owner and identical manifest is idempotent; conflicting ownership or content is rejected. Concurrent identical materialization and startup cleanup are exercised by `concurrent_materialization_is_idempotent_and_recovery_cleans_unready_state`; quota has a dedicated test. |
| Execution identity compatibility | Canonical requests without a workspace remain version 2. A workspace reference uses domain-separated canonical version 3 and participates in the request digest. The core test confirms workspace identity changes the digest. |
| Authorization and capability discovery | Workspace creation is authorized as `WorkspaceCreate`; denied creation returns 403. Node capabilities advertise `workspace.manifest.v1` and `workspace.materialize.v1`. Existing no-workspace execute requests remain deserializable. |

## Verification actually run

Toolchain: repository Rust 1.89.0. Host: Linux. No other OS was exercised.

- `cargo fmt --all` — passed during implementation.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed (40 tests across 4 suites; RTK summary, 90.75s).
- `git diff --check` — passed.

## Compatibility, security, and platform review

- Manifest paths intentionally use a conservative portable ASCII subset. This avoids normalization ambiguity but excludes legitimate non-ASCII filenames.
- Symlinks are unsupported and rejected, so there is no symlink-target escape matrix to qualify. The suite verifies rejection at the manifest boundary, not OS-level link-attack resistance for supported symlinks.
- Linux filesystem behavior and Unix permissions were exercised. Windows and other Unix filesystems are not qualified.
- Tests cover missing blobs and invalid manifests, concurrency, startup reconciliation, exact tree contents, executable mode, owner fencing, and quota. A deterministic injected mid-copy filesystem failure and cleanup test was not added; cleanup after such an error is based on implementation inspection.
- Workspace logical quota accounts for declared file bytes. Database/WAL and filesystem overhead are not included. Workspace deletion and reference-aware retention/GC belong to M003; execution workspaces are retained meanwhile.
- The implementation uses validated path components and fresh private staging directories rather than descriptor-relative traversal APIs. Directories remain node-owned and are not exposed writable to the remote principal.
- No high/medium finding blocks closure.
- Disposition: **closed for the exercised Linux host**, with the platform, filename portability, and mid-copy failure qualifications above recorded as residual limits.

## Registry/roadmap and next-plan disposition

Workspace M002 is closed. Workspace M003's direct dependency is satisfied and it is promoted to active. Security M001 remains ready and stays in the user's requested sequence after Workspace M003. Security M002/M003, Operations M001/M002, and CodeGG M001 remain blocked by their other unmet direct dependencies.
