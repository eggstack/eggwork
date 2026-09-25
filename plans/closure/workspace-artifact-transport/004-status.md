# Workspace and Artifact Transport M004 Closure — Reusable Manifest CAS and Derived Materialization

Source plan: `plans/implementation/workspace-artifact-transport/004-reusable-manifest-cas-and-derived-materialization.md`
Subsystem roadmap: `plans/subsystems/workspace-artifact-transport-roadmap.md`
Reviewed implementation commit: `1e89dac`
Planning/closure commit: *this commit*

## Finding

M004 is closed for the exercised Linux host. A successful full-manifest workspace create now registers its validated canonical manifest in a principal-scoped bounded cache, and a new versioned `workspace.derive.v1` route (`POST /v1/workspaces/derive`) materializes a fresh execution-private workspace from a retained base manifest plus a bounded deterministic patch. The patch applies through a deterministic path map and the final tree passes through the existing `WorkspaceManifest::validate` unchanged, so a derived workspace carries exactly the canonical digest of the equivalent full manifest. The existing schema-v1 `POST /v1/workspaces` contract is untouched, blob-upload semantics are unchanged, no filesystem reflink/hardlink optimization was introduced, and no Git semantics entered Eggwork.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Wire/API compatibility | Existing `POST /v1/workspaces` schema-v1 path is byte-identical in behavior; all pre-existing workspace/blob/artifact tests pass unchanged. The derive capability is a separate route (`POST /v1/workspaces/derive`) and capability string (`workspace.derive.v1`). Client gains `NodeClient::create_workspace_derived(workspace_id, handle, base_manifest_digest, patch)`; the existing `create_workspace` method is unchanged. |
| Canonical patch model | `WorkspaceManifestPatch { schema_version: 1, base_manifest_digest, entries }` and `WorkspacePatchEntry::{Remove, Directory, File}` in `eggwork-core`. Patch validation enforces schema 1, `MAX_WORKSPACE_ENTRIES` bound, and unique patch paths. `apply_to` builds a deterministic `BTreeMap`, treats `Remove` as an exact-path delete (no recursive delete; absent-path remove is a no-op), upserts directory/file entries, then runs the existing `WorkspaceManifest::validate` unchanged and relies on the existing `digest()`. No second path validator exists. Core tests prove duplicate/bounds rejection, order independence, digest equivalence with the equivalent full manifest, remove+upsert semantics, and rejection of orphan-parent, orphaned-child, case-collision, and symlink final trees. |
| Principal-scoped manifest store | `retained_manifests(principal_id, manifest_digest)` table in the WorkspaceManager store; lookups always filter by the transport-derived principal, never by a request payload field (`deny_unknown_fields` on both workspace request types plus a dedicated forged-principal test covering the derive shape). Every successful full or derived create registers/refreshes its canonical manifest before the tree becomes visible. Cross-principal derivation returns the same `409 base_manifest_missing` as an unknown digest, proven at both manager and loopback level. |
| Retention and blob references | Retained manifests pin their file blobs under a distinct `manifest` owner kind with the standard 30-day retention horizon; re-registration extends expiry monotonically (`max(existing, now+horizon)`). Manifest GC (`manifest_garbage_collect`, bounded by limit) deletes expired rows and releases their blob references; workspace GC, artifact GC, and blob GC ordering is workspaces → manifests → artifacts → blobs in both `NodeServer::collect_garbage` and the operator `collect_garbage` path, with matching dry-run previews. A manifest whose blobs become unavailable is reaped on lookup and reported as missing; it can never resurrect expired blob authority. Restart runs `recover_manifest_retention` (NULL expiries become bounded) plus `reconcile_manifests` (malformed rows dropped, expired rows GC'd, orphan `manifest` blob references released), each step bounded to 1..1024 items. A new `BlobStore::reference_owners` helper lists distinct owners without touching blob bytes. |
| Derived materialization flow | `WorkspaceManager::materialize_derived` authenticates via the caller (server authorizes `WorkspaceCreate` for the target workspace id before dispatch), validates schema/handle, resolves the base in-principal-scope with zero side effect on miss, applies the patch deterministically, validates the final manifest, verifies blobs, retains final references, and materializes through the existing `materialize` path (copy from blob CAS, staged dir + rename, owner fencing). The result is a fresh execution-private tree, never an alias. |
| Idempotency and identity | Existing workspace identity/owner fencing is authoritative and shared by both paths. Tests prove same id + owner + final manifest is idempotent under 12-way concurrency, same id with different final digest conflicts (`workspace_identity_conflict`), full-then-derived convergence on one digest without shared ownership, and derived retry returning the identical digest. Execution request-digest/generation/lease semantics are untouched. |
| Failure and fallback contract | Missing/expired base → `409 base_manifest_missing` with no workspace side effect (resolve check in test). Malformed patch shape → `400 invalid_patch`; invalid final tree → `400 invalid_manifest`; missing/corrupt blob, size mismatch, quota, and identity conflict reuse the existing typed codes. The server never falls back to another base; client documents caller-owned full-manifest retry. Authorization/storage/internal errors are never used as fallback signals. |
| No clone optimization | `materialize_tree` still copies file bytes from the CAS through `File::open` + `io::copy`; no hardlink/reflink/shared-writable path was added. |
| Capabilities and diagnostics | `workspace.derive.v1` is advertised in the unified static+dynamic feature snapshot served by both `/v1/capabilities` and `/v1/status`. Bounded counters added: `workspace_derived_requests`, `workspace_derived_hits`, `workspace_base_missing`, `workspace_manifest_registrations`, `workspace_patch_entries`. No manifest path bodies appear in metrics/status. |
| Security review | Forged `principal_id` payload fields are rejected by `deny_unknown_fields` (unit test covers execute, full create, derive, and control shapes). Patch traversal/case/device-name/symlink attempts fail through the shared validator (core tests). Remove/upsert combinations cannot produce a tree the full path would reject (same validator, proven by orphan/collision tests). Derived workspaces are fresh trees under fresh storage keys; base/other-execution trees are never mutated. Blob truthfulness remains SHA-256 verified on upload and re-verified before materialization. |

## Verification actually run

Toolchain: repository Rust 1.89.0. Host: Linux x86_64 (6.8.0-142-generic). Other operating systems were not exercised.

- `python3 scripts/check_execution_ownership.py` — passed (execution ownership guard; no new spawn sites introduced).
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed, no issues.
- `cargo test --workspace --all-targets` — passed (108 passed, 1 ignored, 8 suites).
- `cargo check --workspace` — passed.
- `git diff --check` — passed before commit.
- New tests: 3 core patch tests (`workspace_manifest_patch_*` in `eggwork-core`); 5 manager tests in `workspace.rs` (registration + digest equivalence, cross-principal miss with no side effect, expiry/GC blob release, missing-blob reap + crash-reopen reconcile, 12-way concurrent idempotency + conflict + invalid patch); 2 mTLS loopback tests in `eggwork-server` (`derived_workspace_materialization_loopback`: capability advertisement, full→derived flow, local digest reconstruction equality, full/derived convergence, retry idempotency, unknown-base and cross-principal `base_manifest_missing`, malformed-patch rejection; `derived_workspace_denied_and_oversized_bodies_are_typed`: 403 denial, 413 body bound).
- Pre-existing `terminal_retention_gc_preserves_live_workspace_then_reclaims_inputs` was extended, not weakened: it now asserts the retained manifest keeps pinning the blob after workspace GC and that manifest expiry releases it before blob reclaim.

## Compatibility, security, and platform review

- `POST /v1/workspaces` schema-v1 clients require no migration; the derive route and `workspace.derive.v1` capability are purely additive. `NodeGcReport` gains two additive `u64` fields (`manifest_candidates`, `manifests_deleted`) serialized only to local operator output.
- The manifest cache is bounded by construction: one row per (principal, digest), each row expires on the standard retention horizon, every pass (GC, reconcile, orphan sweep) is capped at 1024 items, and no global indefinite cache exists.
- GC dry-run previews tolerate pre-M004 stores: a missing `retained_manifests` table reports zero manifest candidates instead of failing.
- No Windows/macOS qualification is claimed; artifact capture and workspace materialization platform limits from M002/M003 carry over unchanged.
- No performance qualification or multi-process shared-store qualification was performed; one active node process per state directory remains assumed (enforced by the state lock).
- No high/medium finding in the scoped implementation review blocks closure.

## Registry/roadmap and next-plan disposition

Workspace M004 is closed. Its direct dependent — downstream CodeGG M003 content-aware derived workspace transfer (`dbowm91/codegg: plans/implementation/eggwork-fixed-target-remote-execution/003-content-aware-derived-workspace-transfer.md`) — is unblocked: the `workspace.derive.v1` route, principal-scoped retained manifests, canonical patch contract, and `base_manifest_missing` fallback signal it was registered as waiting on are now closed with loopback evidence above. The downstream plan itself lives in the CodeGG repository and must be handed off there; this record marks the Eggwork-side blocker satisfied.

No other registered plan lists Workspace M004 as a dependency. Security M004 still waits on Operations M002; Operations M003/M004 remain blocked on their stated prerequisites; both are unaffected by this closure.
