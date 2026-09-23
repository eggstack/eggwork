# Workspace and Artifact Transport M001 Closure — Blob Store and Digest Protocol

Source plan: `plans/implementation/workspace-artifact-transport/001-blob-store-and-digest-protocol.md`  
Subsystem roadmap: `plans/subsystems/workspace-artifact-transport-roadmap.md`  
Reviewed implementation commits: `7738920` (verified streaming store and client transport), `f4f18db` (JSON upload metadata preflight), `1f5893f` (oversize preflight test)  
Planning/closure commit: `2aea135`

## Finding

M001 is closed for the exercised Linux host. Eggwork now has a node-owned SHA-256 blob store with a fixed digest-derived path layout, streamed uploads and downloads, content verification, a bounded missing-digest query, quota enforcement, duplicate handling, corruption quarantine, temporary-file cleanup, and metadata needed by later reference tracking and GC.

The client transfer sequence uses JSON control requests for missing-digest discovery and upload preparation (`digest`, `size_bytes`), then sends blob bytes through Eggfetch's streaming request body. The raw PUT also carries the digest in the URL and expected size in its query so the streamed transfer can be bounded and checked independently. The server verifies both declared and observed lengths and the SHA-256 digest before it finalizes a blob.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Canonical SHA-256 identity and safe layout | `BlobDigest::from_bytes` matches the known SHA-256 `abc` vector. Parsing accepts exactly 64 lowercase hex characters and rejects traversal/uppercase inputs. Physical storage maps digest prefix to a fixed two-character directory and the digest to its filename; callers cannot provide a filesystem path. |
| Streamed, bounded transfer | EggServe selects `RequestBodyPolicy::Stream` for valid blob PUT routes with a 256 MiB per-blob ceiling; JSON routes retain small bounded buffers. Eggfetch request streams are sent incrementally. A loopback test uploads and downloads a 2 MiB payload in chunks, larger than the 1 MiB control JSON bound. Downloads are opened only after a full digest/size verification and are returned as bounded 64 KiB file chunks. |
| Control metadata and discovery | `POST /v1/blobs/missing` accepts at most 512 digests in bounded JSON. `POST /v1/blobs/prepare` carries the digest and size in bounded JSON, rejects oversize/quota cases, verifies an existing blob, and tells the client whether upload bytes are needed. |
| Owner-private temporary data and durable finalize | The configured blob root is owner-only (`0700` on Unix); upload parts are created exclusively with mode `0600`. The store hashes while writing, verifies exact declared length and digest, calls file `sync_all`, atomically renames into the digest path, syncs the containing directory on Unix, then commits metadata in SQLite WAL with `synchronous=FULL`. Failed transfers remove their temporary part. |
| Duplicate and concurrent upload behavior | An existing valid digest returns success without replacing bytes. A single store write lock serializes concurrent finalize/quota decisions; a 12-writer same-digest test verifies all callers succeed and exactly one correct content path remains. An HTTP duplicate upload also succeeds after the 2 MiB first transfer. |
| Typed size, digest, quota, and corruption outcomes | Unit and loopback tests exercise wrong observed length, wrong digest (HTTP 422), oversize (preflight 413), quota exhaustion (HTTP 507), and corrupt on-disk content. Corrupt content is moved to a node-owned quarantine directory, removed from the readable index, and fails loudly. |
| Partial transfer and restart cleanup | A simulated body-stream failure after partial bytes leaves no indexed blob or `.part-*` file. Startup removes stale `.part-*` files and reconciles missing/orphaned digest files against metadata. The test writes a crash fixture part, reopens the store, and confirms cleanup. A real TCP disconnect during upload was not separately injected. |
| Authorization | Blob missing/read and upload preparation/write operations use the verified mTLS principal and the configured authorizer before handlers run. Loopback tests verify an authenticated principal denied BlobRead or BlobWrite receives 403. Blob rows are node-wide; per-principal reference ACLs remain for the Security M001 policy and later artifact/workspace milestones. |
| Later GC metadata | The `blobs` table records digest, byte length, `reference_count` initialized to zero, and `last_used_unix_ms`, providing the accounting base for workspace/artifact references and future GC. Committed payload bytes count toward the configured node quota; metadata/database overhead does not. |

## Verification actually run

Toolchain: repository Rust 1.89.0.

- `cargo fmt --all` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed (31 tests across 4 suites; RTK summary, 5.30s).
- `git diff --check` — passed before implementation commits.

## Compatibility, security, and platform review

- Only Linux loopback execution was exercised. Unix private modes and directory fsync are implemented; other Unix platforms and Windows are not qualified by this closure.
- Blob access is authorized per operation using the authenticated transport principal. Blob ownership/reference authorization is not yet modeled; M001 uses node-wide digest storage pending Security M001's authorization policy and later reference-bearing workspace/artifact records.
- The store serializes uploads under one async mutex while streaming. This provides straightforward deterministic quota and same-digest semantics but limits concurrent upload throughput; no performance qualification was performed.
- Stream interruption was tested by injecting a body stream error at the storage boundary, not by forcibly dropping a live TCP/TLS connection. The same storage cleanup branch handles EggServe body read errors.
- The store verifies a full existing blob before serving it and before treating it as a duplicate. Missing discovery uses metadata presence and does not rehash each blob; a later prepare/download detects and quarantines corruption.
- Quota applies to committed blob payload bytes only. SQLite metadata, WAL, quarantine, and filesystem overhead are outside this byte quota. There is no GC yet; that is Workspace M003.
- No high/medium finding blocks closure. No filesystem path from the request is used as a storage path.
- Disposition: **closed for the exercised Linux host**, with the throughput and network-level disconnect qualifications above recorded as residual limits.

## Registry/roadmap and next-plan disposition

Workspace/Artifact M001 is closed. Workspace/Artifact M002's only hard dependency is satisfied and it is promoted to active. Security M001 remains ready and will be reached after the requested workspace sequence. Workspace M003 remains blocked on M002 closure; Security M002/M003, Operations M001/M002, and CodeGG M001 retain their remaining direct dependencies. No later plan is promoted past its unmet dependency.
