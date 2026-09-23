# Workspace and Artifact Transport M003 Closure — Declared Artifacts, Retention, and GC

Source plan: `plans/implementation/workspace-artifact-transport/003-declared-artifacts-retention-and-gc.md`  
Subsystem roadmap: `plans/subsystems/workspace-artifact-transport-roadmap.md`  
Reviewed implementation commit: `59028a1`  
Planning/closure commit: `07f79b8`

## Finding

M003 is closed for the exercised Linux host. Executions capture only explicitly declared output paths. A declared directory is expanded into a bounded tree of regular files; globs, symlinks, and special files are unsupported. Each file becomes a verified SHA-256 blob and an owner-scoped artifact record. Artifact bytes stream through the existing blob transport. Process terminal state and artifact finalization status are separate fields, and artifacts remain outside the compact execution result except for a count.

Workspaces, artifacts, and active artifact readers hold durable blob references. The default retention period is 30 days. A local bounded maintenance API supports dry-run inspection and collection of expired workspaces/artifact records and unreferenced blobs. No remote GC route is exposed in this milestone.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Explicit, bounded outputs | `DeclaredOutput` remains exact-path-only. Existing `MAX_OUTPUTS`, workspace entry/path bounds, a 256 MiB per-blob ceiling, a 4096-file tree ceiling, and a 2 GiB cumulative artifact ceiling bound capture. Tests show undeclared files are omitted and a small storage quota fails capture without publishing records. |
| Confined filesystem capture | Complete paths are discovered and prevalidated before capture. Unix capture opens the root and each path component using descriptor-relative `openat` with `NOFOLLOW`; file hashing and upload use the same opened descriptor. Tests reject final and ancestor symlinks, reject a parent swapped to a symlink after discovery, reject required missing files, and exercise a file disappearing between discovery and opening. |
| Blob integrity and streaming | Capture hashes in 64 KiB chunks, rewinds the same descriptor, and uploads a bounded stream. BlobStore verifies observed length and digest before commit. Retrieval fully verifies the stored blob before opening a streamed response. Client artifact download checks the returned artifact ID and digest headers. The mTLS integration test runs a command that writes an output, lists its record, streams it back, and checks bytes/digest. |
| Artifact metadata and authorization | Records contain ArtifactId, execution ID/generation, principal ownership, relative path, file type, digest, size, executable bit, creation time, and expiry. `ArtifactRead` authorizes listing and retrieval; both metadata queries are owner-fenced and return not-found for other principals. The mTLS denial fixture denies `ArtifactRead`. |
| Terminal ordering and failure semantics | Capture runs after the runner and its output stream finish but before the durable terminal snapshot/event. Succeeded/failed natural process completion may capture; timeout, cancellation, interruption, lease expiry, and output-limit outcomes omit artifacts. A real timeout test confirms partial files are omitted. A required missing output test confirms a successful exit code/state remains visible with `ArtifactCapture` reported separately. |
| Durable references and reader lifetime | SQLite `blob_references` records owner kind/ID/digest/expiry and updates blob reference counts through triggers. Workspace references are registered before materialization and cleared only on preparation rollback. Artifact references are recorded before artifact metadata becomes visible. Active workspace execution clears expiry before reservation; terminalization sets retention, and startup recovers legacy active leases. Stream readers hold renewable expiring references and release them when the stream ends. Shared-reference/live-reader tests prove GC preserves referenced bytes. |
| Retention and bounded GC | `NodeServer::collect_garbage(dry_run, limit)` clamps each pass to 1..1024 items and reports candidate/deleted counts and bytes. It processes expired workspace directories, artifact metadata, expired references, then blobs with no references. Workspace removal and expiry checks serialize in an immediate SQLite transaction. Blob unlink and metadata deletion run under the blob write lock and an immediate SQLite transaction. Tests cover workspace dry-run/expiry, artifact dry-run/expiry, shared blob references, and a restart cut after file unlink where startup reconciles stale blob metadata. |
| Restart and partial cleanup | Workspace reconciliation removes stale staging/untracked directories; blob startup removes partial uploads and drops metadata for missing files. Workspace GC retries missing-directory removal safely. Artifact metadata GC uses a bounded SQLite delete. The test suite directly injects the blob unlink/metadata-commit restart cut; it does not inject process termination at every workspace/artifact GC instruction. |
| Compatibility | Execution results add serde-defaulted finalization status and artifact count. Requests without a workspace remain canonical v2; workspace-bound requests remain v3. Declared outputs require a workspace and are included in the execution digest. Existing clients without artifact methods continue to decode results with defaults. |

## Verification actually run

Toolchain: repository Rust 1.89.0. Host: Linux x86_64. Other operating systems were not exercised.

- `cargo fmt --all` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed (47 tests across 4 suites; RTK summary, 7.68s).
- `git diff --check` — passed before commit.
- Implementation commit `59028a1` was pushed to `origin/main`.

## Compatibility, security, and platform review

- Output capture uses no-follow descriptor-relative operations on Unix. On non-Unix hosts the capture helper fails closed as unsupported; no Windows/macOS artifact capture claim is made.
- Symlink targets and special files are rejected. Directory records are represented by their contained file records; empty directories do not create artifact records. Glob semantics are not implemented.
- Cancellation and timeout omit partial artifacts. Timeout was exercised end to end; cancellation omission is supported by the terminal-state gate and was code-inspected, not separately integration-tested.
- Blob quota covers committed payload bytes, not SQLite/WAL, quarantine, reference metadata, or filesystem overhead. The bounded GC pass may require repeated runs to process more than one batch.
- Artifact retrieval is principal-fenced. Raw digest retrieval continues to use the node's existing `BlobRead` operation policy; fine-grained workspace/artifact authorization is part of Security M001.
- No performance qualification or multi-process shared-store qualification was performed. This service continues to assume one active node process owns its configured stores.
- No high/medium finding in the scoped implementation review blocks closure. Process isolation/resource enforcement and the broader authorization/redaction review remain in the scheduled Security milestones.
- Disposition: **closed for the exercised Linux host**, with the platform, cancellation test, GC cut-point, quota-overhead, and performance qualifications above recorded as residual limits.

## Registry/roadmap and next-plan disposition

Workspace M003 is closed. Security M001 is promoted from ready to active in the user's requested sequence. Security M002 is ready because Foundation M002 and Workspace M002 closures satisfy its direct dependencies; it remains sequenced after M001. Security M003 remains blocked on Security M002 closure. Operations M001 and CodeGG M001 remain blocked only on Security M003; Operations M002 also retains the stable Eggup consumer-interface dependency.
