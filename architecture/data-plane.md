# data plane

The data plane is the node's byte-moving half: three private modules inside `eggwork-server` that
store content-addressed blobs, reconstruct a portable file tree from a manifest, and capture the
declared outputs of a finished execution. `blob.rs` owns immutable, deduplicated bytes addressed by
SHA-256. `workspace.rs` turns a validated manifest into an execution-private directory tree and
knows how to derive a new tree from a retained base plus a small patch. `artifact.rs` reads back
only the outputs an execution declared, stores them as blobs, and records their metadata with an
expiry. All three share one discipline: the digest is the identity, a name means nothing until its
bytes have been hashed and checked, and any violation of a bound, a path rule, or a digest is
refused rather than repaired. The normative basis is
[ADR-0004](../plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md).

## Responsibility boundary

**Owns**

- Storage and integrity of file bytes: ingest, verification, dedup, streaming read, reference
  accounting, reclamation (`BlobStore`).
- Materialization of a validated `WorkspaceManifest` into a real directory tree, every path-safety
  rule, the quota check, the staging/rename protocol, retention of the tree, and a small
  principal-scoped cache of canonical manifests used by `workspace_derive` (`WorkspaceManager`).
- Capture of declared outputs from an execution root into blobs, plus the artifact metadata rows
  and their expiry (`ArtifactStore`, `capture_declared`).
- Node-local quotas over the two directory trees it owns (`blob_quota_bytes`,
  `workspace_quota_bytes`).

**Does not own**

- Execution lifecycle. Nothing here decides that an execution should start, that it is terminal,
  or which command runs. `mark_active` / `mark_terminal` / `recover_retention` are storage-side
  retention transitions the executor calls, not decisions.
- Admission, leases, generation fencing, scheduling. Fencing *inputs* arrive as arguments
  (`ExecutionHandle`, `PrincipalId`); the data plane only refuses a mismatch.
- Authorization. The HTTP layer calls `authorize_resource(...)` before touching a store; the
  stores are principal-fenced by SQL predicates, not by a policy engine.
- The execution journal. The journal holds `artifact_count` and `finalization_failure`; artifact
  record bodies live in the artifact store's own SQLite file.
- Anything cross-host. All three roots are node-local: no remote filesystem path, no
  absolute-path input, no Git integration (ADR-0004 decisions 1-3, 9, 14).

All three are crate-private modules with no `pub use` re-export, so nothing outside
`eggwork-server`'s own modules can name `BlobStore`, `WorkspaceManager`, or `ArtifactStore`.

## Shared discipline

**Content addressing.** Every byte object is named by a `BlobDigest`: exactly 64 lowercase hex
characters, produced by `BlobDigest::from_bytes` (SHA-256) and re-checked by `BlobDigest::parse`
(`crates/eggwork-core/src/lib.rs:141-160`). The server turns a digest into a path mechanically; it
never derives a name from anything the caller controls. Because `path_for` is a pure function of
the digest, a digest that survives `BlobDigest::parse` cannot name a location outside the blob
root: `"../secret"` and uppercase hex are rejected before a path is built (`blob.rs:665`).

**Immutability.** A committed blob is never rewritten: `put_stream` refuses the write when the
target exists and runs `verify_existing` instead (`blob.rs:408-410`). The only writes touching a
live path are: create the shard directory, create `.part-<uuid>`, `rename` the part onto the final
name.

Immutability is what makes deduplication safe. Because a name implies fixed content, a second
upload of the same digest is answered without transferring bytes (`find_missing` /
`prepare_upload` returning `upload_required: false`), and one blob can back many workspaces and
artifacts with no copy-on-write and no version negotiation. If bytes under a live name could
change, every holder would silently observe a different file, and a digest verified at ingest would
mean nothing at read time.

**Write-once staging.** Blob ingest writes `.part-<uuid v4>` at the blob root, `fsync`s it, renames
onto the final path, `fsync`s the shard directory, and only then inserts the index row
(`blob.rs:427-466`). Workspace trees are built in `.staging-<uuid v4>` and renamed to
`workspace-<uuid v4>` (`workspace.rs:230-292`). A reader never observes a partially written object;
the rename is the single atomic visibility edge.

**Quota enforcement.** Each tree has a byte budget in its own metadata: the blob store sums
`blobs.size_bytes`, the workspace store sums `workspaces.logical_bytes` for rows in state `'Ready'`.
Both refuse with a distinct typed error (`BlobError::QuotaExceeded` /
`WorkspaceError::QuotaExceeded`) rather than a truncated write.

**Fail-closed.** There is no best-effort path. A body whose hash does not match is deleted, a
manifest that fails validation writes nothing, a symlinked output is refused rather than copied, a
blob that fails re-verification is quarantined and reported, and a missing required output fails the
whole capture instead of producing a partial record set.

## Blob store (`blob.rs`)

### On-disk layout

```
<blob_root>/                                mode 0700 on unix (blob.rs:74-78)
  metadata.sqlite                           blobs + blob_references (blob.rs:82-110)
  metadata.sqlite-wal / -shm
  quarantine/                               corrupt blobs, under the 0700 root (blob.rs:79-80)
  .part-<uuid v4>                           in-flight uploads only; never a valid object name
  <d0d1>/<64-hex-digest>                    one object, mode 0600 (blob.rs:631-640)
  <d2d3>/...                               one shard directory per first-hex-byte pair
```

`path_for` is `root.join(&digest.as_str()[..2]).join(digest.as_str())` (`blob.rs:123-128`): a
single level of sharding on the first two hex characters - 256 shard directories, no deeper
nesting, no extension, full digest as the file name. Two-digit sharding keeps any one directory
from accumulating the whole store while leaving `read_dir` cheap at open time (`remove_stale_parts`
walks exactly the shards).

The metadata database, not the filesystem, is the authority for existence. `blobs` is
`(digest TEXT PRIMARY KEY, size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
reference_count INTEGER NOT NULL DEFAULT 0, last_used_unix_ms INTEGER NOT NULL)`;
`blob_references` is `(owner_kind, owner_id, digest, expires_unix_ms)` keyed on
`(owner_kind, owner_id, digest)` with `FOREIGN KEY(digest) REFERENCES blobs(digest) ON DELETE
CASCADE`. `reference_count` is never written by Rust - two triggers (`blob_reference_insert` /
`blob_reference_delete`, `blob.rs:101-110`) maintain it. Reclamation queries deliberately use
`NOT EXISTS (SELECT 1 FROM blob_references ...)` rather than the counter.

`blob_references.owner_kind` is a small closed set in practice: `"workspace"`, `"manifest"`,
`"artifact"`, and `"reader"` (in-flight download, `lib.rs:1413`). `owner_id` is the `WorkspaceId`,
the `"{principal}:{manifest_digest}"` composite (`workspace.rs:883`), the `ArtifactId`, or a fresh
UUID.

### Public surface

- `MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024` (`blob.rs:19`) - per-object ceiling.
- `MAX_FIND_DIGESTS: usize = 512` (`blob.rs:20`) - per-request digest batch ceiling.
- `GcReport { expired_references, expired_references_removed, candidate_blobs, candidate_bytes,
  removed_blobs, removed_bytes }` (`blob.rs:23-30`). No `dry_run` field: the caller supplies and
  records that (`NodeGcReport.dry_run`, `lib.rs:507`).
- `BlobError` (`blob.rs:33-55`), walked in [Error model](#error-model).
- `BlobStore::{open, path_for, find_missing, prepare_upload, stored_size, retain,
  set_reference_expiry, clear_reference_expiry, release_references, reference_owners,
  garbage_collect, put_stream, verify_existing, open_verified}`. `BlobStore` is `Clone` and
  internally `Arc`-shared, so one instance is reachable from every handler and from the workspace
  and artifact modules.

### Ingest

`POST /v1/blobs/prepare` calls `prepare_upload(digest, declared_length)` (`blob.rs:153-179`):
reject `> MAX_BLOB_BYTES`; if the target path already exists, run `verify_existing` and answer
`upload_required: false`; otherwise do a metadata-only quota pre-check and answer
`upload_required: true`. `POST /v1/blobs/{digest}` calls `put_stream` (`blob.rs:394-493`).

The digest is **declared by the client and verified by the node**. `put_stream` never trusts it: it
hashes every chunk as it writes, so the digest is computed exactly once, from the bytes actually
received, and compared at the end.

Order inside `put_stream`: (1) `declared_length > MAX_BLOB_BYTES` -> `TooLarge`; (2) acquire
`write_lock`, a `tokio::sync::Mutex` held for the *entire* upload, so blob writes are serialized
node-wide; (3) target exists -> `verify_existing` and return, no write; (4) quota, `SUM(size_bytes)
+ declared_length > quota` -> `QuotaExceeded` - the bound uses the *declared* length, so a client
cannot overshoot by under-declaring; (5) `create_dir_all` the shard and create the part file with
`private_create_new` (mode 0600, `blob.rs:631-640`); (6) stream chunks, `checked_add`ing size and
returning `TooLarge` if `size > MAX_BLOB_BYTES || size > declared_length`; (7) `size !=
declared_length` -> `LengthMismatch`; (8) `hex::encode(hasher.finalize()) != digest.as_str()` ->
`DigestMismatch`; (9) `sync_all`, `rename(part, target)`, `sync_directory(shard)`, then
`INSERT INTO blobs`.

On any error the part file is removed, and if the target exists but has no index row the target is
removed too - the comment at `blob.rs:469-470` states the rule directly: *a failed metadata commit
must not leave a readable unindexed blob*.

Note an asymmetry worth flagging: an *over*-long body is reported as `TooLarge` (413), not
`LengthMismatch` (422), so a client that streams more than it declared may conclude the object is
too big to upload when the correct action is to re-declare.

**Partial uploads are not observable.** The part file lives at the blob root - never in a shard,
never in `blobs` - so `path_for(digest)` does not exist, `find_missing` reports the digest as
missing, `open_verified` returns `NotFound`, and the HTTP layer answers 404. Aborted part files are
swept at the next `BlobStore::open` by `remove_stale_parts` (`blob.rs:557-613`), which also
reconciles the two durable layers in both directions: index rows whose file is gone (or whose digest
no longer parses) are deleted, and shard files that are not indexed are removed.
`remove_stale_parts` consults neither `last_used_unix_ms` nor references, so it would delete a live
concurrent upload's part file; only one `BlobStore` is constructed per process, at startup
(`lib.rs:318`).

### Read

`open_verified` (`blob.rs:552-555`) calls `verify_existing` first, then opens the file.
`verify_existing` (`blob.rs:495-550`) re-reads the entire object in `spawn_blocking`, compares the
byte count against `blobs.size_bytes` and the fresh SHA-256 against the digest, and on a clean pass
updates `last_used_unix_ms` to now.

Digests are therefore **verified on every read**, not only at ingest. The index is not trustworthy
on its own: a crash between `rename` and the metadata commit, external file loss, or a quarantine
event can all desynchronize the two layers, and a blob that silently served the wrong bytes would
poison every workspace and artifact derived from it. The cost is that blob download and workspace
materialization both pay a full re-hash of every input.

On mismatch the file is `rename`d to `quarantine/<digest>-<uuid v4>`, the index row is deleted,
and `CorruptExisting` is returned. Quarantined bytes are never re-read and are not counted against
the quota, but they are not deleted by GC either - they are left for an operator.

### Quota

Two enforcement points with different authority. `prepare_upload` (`blob.rs:165-178`) is advisory,
lock-free, metadata-only - a fast "do I need to transfer this?" answer that can be stale by the
time the upload starts. `put_stream` (`blob.rs:411-422`) is authoritative, under `write_lock`; that
is the real bound. A client that skips `prepare` still gets a correct 507.

`SUM(size_bytes)` counts committed, indexed objects only, so part files, quarantined objects, and
unindexed strays sit outside the accounting. There is no filesystem-level backstop on the blob
root; `storage_summary` only *reports* `blob_bytes` against `blob_quota_bytes`
(`operations.rs:770-781`). At the limit the store refuses the new object with `QuotaExceeded` ->
507; existing objects are never evicted to make room.

### GC

`garbage_collect(now_unix_ms, limit, dry_run)` (`blob.rs:302-392`) takes `write_lock` and an
`IMMEDIATE` transaction, so it is serialized against `put_stream` (same lock) though not against
`retain` (metadata mutex only).

Two phases. First, an unbounded `COUNT(*)` of expired references
(`expires_unix_ms IS NOT NULL AND expires_unix_ms <= now`) - the number a dry run reports. Then,
when not a dry run, delete up to `limit` of them, **re-select** blob candidates, and delete them.
Selection is `WHERE NOT EXISTS (SELECT 1 FROM blob_references r WHERE r.digest = blobs.digest)
ORDER BY last_used_unix_ms ASC LIMIT ?1` (`blob.rs:318-324`): unreferenced-only, cold
(least-recently-verified) first. Selection is repeated after the expired references are dropped so
a blob that just became unreferenced is eligible in the same pass.

Per candidate: `fs::remove_file`, then
`DELETE FROM blobs WHERE digest = ?1 AND NOT EXISTS (SELECT 1 FROM blob_references WHERE digest =
?1)`. `NotFound` on the unlink still removes the row; any other unlink error aborts the whole
transaction (`blob.rs:377`), leaving the store consistent.

GC is **bounded, not exhaustive**: one call removes at most `limit` expired references *and* at
most `limit` blobs. A backlog drains over repeated invocations, and
`NodeServer::collect_garbage` clamps `requested_limit` to `1..=1024` (`lib.rs:464`) while calling
each store once.

### Reference protection during download

`BlobReferenceLease` (`lib.rs:1373-1392`) is a `Drop` guard over a `(BlobStore, owner_id)` pair
whose `renew()` calls `set_reference_expiry("reader", owner_id, now + 30 min)` and whose
`Drop` calls `release_references("reader", owner_id)`.

`artifact_download` (`lib.rs:1409-1452`) retains `"reader"` for the artifact's digest with a
30-minute expiry *before* opening the file, constructs the lease, and moves the lease into the
response stream's unfold state; each 64 KiB chunk calls `renew()`, so the reference is refreshed as
the transfer progresses, and it is dropped as soon as the body is dropped - completion, failure, or
client disconnect. If the initial `retain` fails the handler answers 404 rather than streaming
unreferenced bytes.

Why it is required: `verify_existing` and `open_verified` run *before* any bytes are sent, and
`BlobStore::garbage_collect` takes only `write_lock`, which an already-open download does not hold.
Without a reference row a concurrent GC pass could select and `remove_file` the object between
verification and the first `read`, and the transfer would fail mid-body with no recovery. The lease
turns "a file I am currently streaming" into a fact GC can see.

Note the asymmetry: `blob_download` (`lib.rs:1306-1336`) takes **no** lease. It verifies and opens
without a reference. On Unix the already-open descriptor survives `unlink`, so an in-flight blob
download keeps working even if GC removes the path; on other platforms removing an open file fails
and the GC aborts. The `reader` owner kind is used only by the artifact handler - a reviewer should
decide whether that asymmetry is intended.

## Workspace manager (`workspace.rs`)

### Manifest and entry interpretation

`WorkspaceManifest { schema_version: u16, entries: Vec<WorkspaceEntry> }` with

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEntry {
    Directory { path: RelativePath },
    File { path: RelativePath, digest: BlobDigest, size_bytes: u64, executable: bool },
    Symlink { path: RelativePath, target: String },
}
```

(`crates/eggwork-core/src/lib.rs:590-611`). `schema_version` must be `1`. The doc comment at
`core:587-588` gives the intent: version 1 uses conservative ASCII paths and rejects symlinks so
materialization is identical across hosts.

`WorkspaceManifest::validate` (`core:622-690`) enforces, in order: `schema_version == 1`;
`entries.len() <= MAX_WORKSPACE_ENTRIES`; per entry, `validate_portable_workspace_path`, aggregate
path bytes `<= MAX_WORKSPACE_PATH_BYTES`, ASCII case-collision detection
(`to_ascii_lowercase()` into a `HashSet`), and no duplicate exact path; `Symlink` ->
`ValidationError::Forbidden` unconditionally (`core:667-671`), so a symlink can never appear in a
valid v1 manifest; `File` sizes accumulate into a `u64` capped at `MAX_WORKSPACE_LOGICAL_BYTES`.
A second pass then requires every parent prefix of every path to be present as a `Directory` entry
(`core:673-689`), so a `File` can never implicitly create a directory.

`WorkspaceManifest::digest` (`core:702-712`) sorts entries by path, serializes
`eggwork-workspace-manifest\0canonical-json-v1\0` + the normalized JSON, and takes SHA-256. This is
the workspace identity used for idempotency, conflict detection, and the derived-base cache.

`validate_portable_workspace_path` (`core:828-878`) is the strict validator, and it is stricter than
`RelativePath::new` (`core:189-210`). `RelativePath::new` enforces: non-empty, at most
`MAX_PATH_BYTES` (4096) bytes, no leading `/`, no `\`, no NUL, and no empty, `.` or `..` component.
The strict validator adds: at most `MAX_WORKSPACE_DEPTH` (128) components; ASCII only; no component
ending in `.` or a space; no C0 control byte and none of `<`, `>`, `:`, `"`, `?`, `*`; and no
`CON`/`PRN`/`AUX`/`NUL`/`CONIN$`/`CONOUT$`/`CLOCK$` basename and no `COM1`-`COM9` or `LPT1`-`LPT9`.
The stricter set exists for portability to case-insensitive and Windows hosts. The weaker set is all
that `DeclaredOutput.path` and the discovered artifact paths get; the gap is closed for artifacts
because `capture_declared` runs the discovered tree back through `WorkspaceManifest::validate` as a
preflight.

### `ReadyWorkspace`

`ReadyWorkspace { root: PathBuf, logical_bytes: u64, manifest_digest: String }`
(`workspace.rs:62-66`) is an ephemeral handle to an on-disk tree, not a guard or a lock: it holds
no reference, no permit, and does nothing on drop. `root` is the absolute path the runner is handed
as the execution root (`lib.rs:1617-1643`: with a `workspace_id` the root is
`workspaces.resolve(...).root`, otherwise `state.execution_root`).

Its lifetime is the *workspace record*, not the function that produced it. `materialize` returns
one, `resolve` re-derives one later, and the tree persists until retention expiry plus GC. The
executor pins the record at admission with `mark_active` (`expires_unix_ms = NULL`, blob references
set to non-expiring) and unpins it at terminalization with `mark_terminal`; a failure to pin is
reported as 409 `workspace_not_ready` (`lib.rs:1742-1753`).

### Materialization safety

This is the most security-critical path in the node: a hostile manifest arrives over the network and
becomes real files on disk.

**Pre-flight, not interleaved.** `materialize` (`workspace.rs:131-308`) validates everything before
any directory exists: (1) `manifest.validate()` -> `InvalidManifest`; (2) a single manifest's
`logical_bytes` against the whole quota -> `QuotaExceeded`; (3) canonical `manifest.digest()`;
(4) `materialize_lock`, a `tokio::sync::Mutex` serializing materialization node-wide; (5)
idempotency - an existing `workspace_id` must match on `execution_id`, `generation`,
`principal_id`, and `manifest_digest` and must not be expired, else `Conflict`, and a match returns
the existing tree without re-verifying any bytes; (6) aggregate quota over
`SUM(logical_bytes) WHERE state = 'Ready'`; (7) **a pre-flight blob check for every `File` entry,
before any write** (`workspace.rs:184-201`): `size_bytes > MAX_BLOB_BYTES` ->
`BlobSizeMismatch`, `stored_size(digest)` must equal `size_bytes`, then
`blobs.verify_existing(digest).await` - a full re-hash of every input before a single byte is
written; (8) deduplicate the digests and
`blobs.retain("workspace", workspace_id, digests, Some(now + DEFAULT_RETENTION_MILLIS))` -
reference first, bytes second; (9) `store_manifest(...)` registers the canonical manifest as a
same-principal derived base, pinning the same blobs under owner kind `"manifest"`
(`workspace.rs:223-228`).

Only then does it create `.staging-<uuid v4>`, run `materialize_tree` in `spawn_blocking`, rename to
`workspace-<uuid v4>`, `sync_directory(root)`, and insert the `'Ready'` row. Every failure branch
removes the stage (or the freshly renamed target) *and* calls
`release_references("workspace", ...)`. The `store_manifest` registration is deliberately **not**
rolled back (`workspace.rs:223-227`): a leftover expiring cache entry is harmless, whereas unpinning
a blob another live workspace is using is not.

Pre-flight is the right choice because the alternative - validating while writing - means a manifest
failing on its 4096th entry leaves a partially written tree. Staging already provides the visibility
guarantee, so validating first is nearly free: the tree is invisible either way, and the pre-flight
loop fails without touching the filesystem at all.

**Inside the staging tree** (`materialize_tree`, `workspace.rs:887-939`): `fs::create_dir(stage)`
(fails if the name exists) then `set_directory_permissions` to 0700; entries are **sorted by path**
before creation, so parents precede children with no dependency analysis, and parent existence is
already guaranteed by the second pass of `manifest.validate()`. `Directory` -> `create_dir` + 0700.
`File` -> `OpenOptions::new().write(true).create_new(true)` with `mode(0o700)` when `executable` else
`0o600`, copying from `blobs.path_for(digest)` opened with `File::open`; `io::copy` must yield
exactly `size_bytes` else `BlobSizeMismatch` (`workspace.rs:919-921`), then `sync_all()` and
`set_file_permissions`. `Symlink` -> `InvalidManifest` (`workspace.rs:926`) - defense in depth,
since core already rejected it. Afterwards every created directory is `sync_all`ed deepest-first
(reverse component count).

**Symlink policy: materialization never creates a symlink.** No code path in `materialize_tree`
calls `symlink`, and a symlinked *parent* cannot exist because the staging root is fresh, 0700, and
every subdirectory comes from `create_dir` (which fails on an existing entry). Traversal is
impossible because `RelativePath` has no `..`, no absolute prefix, and no `\`; and
`stage.join(entry.path().as_str())` is a plain `Path::join` on a path the strict validator already
constrained to portable ASCII.

**Resolution re-checks the root** (`resolve`, `workspace.rs:755-786`): the record must be
`state = 'Ready'`, match `execution_id`, `generation`, and `principal_id`, not be expired, satisfy
`valid_storage_key`, and have a `fs::symlink_metadata` that is a directory and *not* a symlink.
`valid_storage_key` (`workspace.rs:877-881`) requires the prefix `workspace-` followed by a
parseable UUID, which is what stops a hostile or corrupted `storage_key` column from turning
`root.join(storage_key)` into a path outside the workspace root. `mark_active`
(`workspace.rs:631-660`) repeats the `valid_storage_key` and `symlink_metadata` checks under an
`IMMEDIATE` transaction before clearing the expiry.

### `workspace_derive`: manifest CAS and reuse

`POST /v1/workspaces/derive` (`lib.rs:1125-1250`) is a bandwidth optimization, not a different
security model. It carries `WorkspaceManifestPatch { schema_version, base_manifest_digest, entries }`
with `WorkspacePatchEntry::{Remove{path}, Directory{path}, File{path, digest, size_bytes,
executable}}` (`core:720-747`). `Remove` deletes exactly one path - there is no recursive delete, so
removing a directory requires explicit descendant removals - and rename is `Remove` + upsert.
Ordering never affects the result because `apply_to` (`core:784-823`) folds into a
`BTreeMap<String, WorkspaceEntry>`.

`materialize_derived` (`workspace.rs:309-327`):

1. `patch.validate()` -> `InvalidPatch` (bounded by `MAX_WORKSPACE_ENTRIES`, unique paths).
2. `lookup_manifest(principal_id, &patch.base_manifest_digest, blobs)` -> `BaseMissing` on
   absence. The lookup is keyed on `(principal_id, manifest_digest)`, so a base retained for
   another principal is indistinguishable from one that does not exist - manifest existence is not
   a cross-principal information channel (plan 004, section 5).
3. `patch.apply_to(&base)` -> `InvalidManifest`. The final manifest goes through the *existing*
   `WorkspaceManifest::validate`, and its identity is the *existing* `WorkspaceManifest::digest`.
   There is deliberately no second path validator (plan 004, section 4).
4. `self.materialize(workspace_id, handle, principal_id, final_manifest, blobs)` - the ordinary
   path, with the full pre-flight.

How content avoids re-transfer: the base manifest body lives in
`retained_manifests(principal_id, manifest_digest, manifest_json, logical_bytes, created_unix_ms,
expires_unix_ms)` (`workspace.rs:93-100`) and its blobs stay pinned under owner kind `"manifest"`
for the retention horizon. A derived request therefore carries only the patch plus digests for
genuinely new files; unchanged files are already in the store and are never re-uploaded.

Identity of the derived manifest is the canonical digest of the *final* manifest, so a full create
and a derived create converge on the same `manifest_digest` and `ReadyWorkspace.logical_bytes`
without sharing a directory (`workspace.rs:1310`, and over the wire `lib.rs:4249`).

`store_manifest` (`workspace.rs:333-404`) re-validates the manifest and the digest format,
serializes the body, and upserts with **monotonic** expiry (`max(existing, now +
DEFAULT_RETENTION_MILLIS)`), so re-registration extends retention and never shortens it.
`lookup_manifest` (`workspace.rs:406-454`) treats a base as unusable - and reaps it - when the JSON
fails to parse, validation fails, or any referenced blob's stored size is missing or mismatched
(`workspace.rs:444-449`); an expired base is reported as `None` without deletion. A base is never
served after its blobs are gone.

`reconcile_manifests` (`workspace.rs:535-611`) and `recover_manifest_retention`
(`workspace.rs:518-533`) run at startup: expired manifests are dropped with their references, and
any `"manifest"` owner id in `blob_references` with no matching `retained_manifests` row is released
via `blobs.reference_owners("manifest", limit)`.

### Quota and limits

`workspace_quota_bytes` (`OperatorConfig`) is passed to `WorkspaceManager::open`
(`lib.rs:320-322`). The manifest-shape limits - `MAX_WORKSPACE_ENTRIES`, `MAX_WORKSPACE_PATH_BYTES`,
`MAX_WORKSPACE_DEPTH`, `MAX_WORKSPACE_LOGICAL_BYTES` - are enforced in core, not here; the only
server-side per-entry size cap is `MAX_BLOB_BYTES` at `workspace.rs:189-191`.
`MAX_WORKSPACE_REQUEST_BYTES = 4 MiB` (`lib.rs:54`) bounds the buffered derive body.

Quota counts `state = 'Ready'` rows only, so a workspace whose record is terminal-but-not-yet-GCed
stops counting toward the quota while still occupying disk. That is deliberate - the workspace is
no longer usable, so a new one may proceed - but it means `storage_summary.workspace_bytes` (a
filesystem walk) can exceed what the quota accounted for.

## Artifact store (`artifact.rs`)

### `capture_declared`

`capture_declared(store, blobs, root, outputs, execution_id, generation, principal, expires_unix_ms)`
(`artifact.rs:225-323`) is called from the execution finalizer, never by the client. Eligibility is
decided entirely by what the execution declared: `WorkspaceCapture { id, root, principal, outputs }`
is built from `wire.spec.command.declared_outputs` (`lib.rs:1635-1640`), and capture runs only when
`runner_completed && !outputs.is_empty() && state ∈ {Succeeded, Failed} && failure != OutputLimit`
(`lib.rs:2445-2452`). A timed-out or cancelled execution captures nothing, and one killed for output
limit is deliberately excluded.

Flow: `discover_outputs` runs in `spawn_blocking` (`artifact.rs:419-469`) and first requires the
execution root itself to be a directory and not a symlink. For each
`DeclaredOutput { path: RelativePath, required: bool }`, `safe_metadata_under_root`
(`artifact.rs:471-489`) walks the path component by component with `fs::symlink_metadata` and fails
on **any** symlink component or any non-directory parent; `NotFound` on a non-required output skips
it, `NotFound` on a required one is `MissingRequired`. `visit_output` (`artifact.rs:491-547`) then
recurses: a symlink is `UnsupportedType`, a non-file/non-dir is `UnsupportedType`, files are
recorded with their executable bit, and child logical paths are derived by `strip_prefix(root)`,
never by string concatenation from the declared path. Bounds are checked both during recursion
(`files + directories > MAX_ARTIFACT_FILES`, `artifact.rs:539-541`) and after the loop
(`artifact.rs:451-453`).

A preflight `WorkspaceManifest` built from the discovered files (zero digest, zero size) plus the
discovered directories is passed to `WorkspaceManifest::validate` -> `InvalidTree`
(`artifact.rs:245-262`). This is the reuse-the-strict-validator trick: the discovered tree is held
to the same portable path rules as a workspace.

Per file, `capture_blob` (`artifact.rs:325-377`) opens the file **descriptor-relatively** with
`open_file_beneath` (`artifact.rs:379-410`, unix): `open(root, O_RDONLY|DIRECTORY|CLOEXEC|NOFOLLOW)`,
then `openat(..., O_NOFOLLOW)` per parent, then
`openat(final, O_RDONLY|CLOEXEC|NOFOLLOW|NONBLOCK)`. It hashes while reading, enforcing both
`MAX_BLOB_BYTES` and the remaining capture budget (`MAX_ARTIFACT_TOTAL_BYTES - total`) -> `Limit`,
then `seek(Start(0))` and streams into `blobs.put_stream`, so the bytes are hashed and verified
again. The `NOFOLLOW` chain is the strongest symlink defense in the codebase: a symlink swapped in
mid-walk cannot be followed even if the workspace can create one. On non-unix platforms
`open_file_beneath` returns `ErrorKind::Unsupported`, so the safe path is unavailable rather than
silently replaced by a path-based open.

The real `WorkspaceManifest` is then rebuilt with actual digests and validated again, `blobs.retain`
is called per record under owner kind `"artifact"` **before** the rows become visible
(`artifact.rs:311-321`), and only then does `store.insert_many` publish them. The comment at
`artifact.rs:312-313` states the asymmetry: a crash here can retain extra bytes until expiry, never
lose live data.

**Where artifact metadata lives: the artifact store's own SQLite file, not the journal.**
`ArtifactStore::open` (`artifact.rs:63-97`) opens `<database_path>.with_extension("artifacts.sqlite")`
(`lib.rs:323`) with `WAL` and `synchronous=FULL`, creating the parent directory and the file with
mode 0600 on unix. The table is

```sql
artifacts(artifact_id TEXT PRIMARY KEY, execution_id TEXT NOT NULL, generation INTEGER NOT NULL,
          principal_id TEXT NOT NULL, record_json BLOB NOT NULL,
          created_unix_ms INTEGER NOT NULL, expires_unix_ms INTEGER NOT NULL,
          UNIQUE(execution_id, generation, principal_id, record_json))
```

plus `artifacts_owner ON artifacts(execution_id, generation, principal_id, created_unix_ms)`. The
full `ArtifactRecord` (`core:957-968`) is stored as JSON in `record_json`; `artifact_id`,
`execution_id`, `generation`, `principal_id`, and both timestamps are also projected into columns
for querying. The `UNIQUE` constraint makes re-inserting an identical record set a constraint error,
so `insert_many` is not silently idempotent. The execution journal carries only the aggregate:
`ExecutionResult.artifact_count` and `finalization_failure` (`lib.rs:2459-2474`).

`ArtifactType` has exactly one variant, `File` (`core:970-974`). Directories are not artifacts;
they exist only as entries in the capture preflight manifest so the tree validates.

### Retention

`DEFAULT_RETENTION_MILLIS = 30 * 24 * 60 * 60 * 1000` = 30 days (`artifact.rs:22`). The store owns
neither the clock nor the horizon: the caller computes
`now_unix_ms().saturating_add(DEFAULT_RETENTION_MILLIS)` and passes `expires_unix_ms` in
(`lib.rs:2454`, and again at `lib.rs:2478` for `mark_terminal`). `now_unix_ms` is exported
(`artifact.rs:564`) precisely so all three modules share one clock.

Past retention, an artifact is **invisible immediately and reclaimed later**. Both `list`
(`artifact.rs:126-159`) and `get` (`artifact.rs:161-184`) filter `expires_unix_ms > now`, so an
expired artifact cannot be listed or downloaded the moment it expires; the row and the bytes survive
until a GC pass removes them. `artifact_list` (`lib.rs:1340`) is also principal-scoped and requires
an explicit `generation` query parameter.

The workspace side uses the same 30-day horizon: `materialize` sets the initial expiry, `mark_active`
clears it to `NULL` (live, no expiry), `mark_terminal` sets it, and `recover_retention`
(`workspace.rs:667-696`) re-arms every `NULL` workspace at `now + DEFAULT_RETENTION_MILLIS` during
startup, so a crash mid-execution cannot leave a permanently live reference.

### Bounded cleanup

`ArtifactStore::garbage_collect(now_unix_ms, limit, dry_run)` (`artifact.rs:186-223`) runs three
bounded statements: count all expired, count the bounded candidate window (`ORDER BY
expires_unix_ms LIMIT limit`), and - when not a dry run - delete that window.
`ArtifactGcReport { candidate_artifacts, deleted_artifacts }` (`artifact.rs:54-57`), where
`deleted_artifacts` is the bounded candidate count, not a measured row count.

"Stop at limit" is the important semantic: **one pass never drains a backlog.** Both the artifact
and blob collectors are `LIMIT`-bounded per invocation, and `NodeServer::collect_garbage`
(`lib.rs:460-508`) calls each store exactly once with a clamped limit of `1..=1024`. A node with
50,000 expired artifacts needs repeated invocations - scheduled or operator-driven - to reclaim
space. Expiry is therefore *prompt* (reads stop at expiry) but *reclamation* is eventual and
interruptible, and disk usage between passes is bounded by nothing but the filesystem.

Artifact GC also does **not** release blob references. The `"artifact"` reference rows expire
independently (same horizon, stored on the row) and are swept by the blob store's expiry phase;
only then do the bytes become candidates for the unreferenced-blob pass. Reclaiming one artifact's
bytes needs both collectors, in the right order - which `NodeServer::collect_garbage` guarantees by
calling artifact GC before blob GC.

### Interaction with blob GC and the live `collect_garbage` path

`NodeServer::collect_garbage(dry_run, requested_limit)` (`lib.rs:460-508`) is the single entry
point. In order: workspace GC, retained-manifest GC, artifact GC, blob GC - all with the same `now`
and the same `limit` - folded into `NodeGcReport`. For a dry run it reports
`expired_blob_references` (the unbounded count); for a real run `expired_references_removed` (the
bounded number actually deleted). The order matters: workspace GC deletes trees and rows first,
which is what makes those blobs unreferenced; artifact GC deletes rows next, which is what makes the
reference rows expirable; blob GC runs last and picks up everything that became collectable. The
blob pass is the only one that takes `BlobStore::write_lock`, so it is the one that can block behind
an in-flight upload.

## Bounds and quotas

| Constant | Value | Location | What it bounds |
| --- | --- | --- | --- |
| `MAX_BLOB_BYTES` | `256 * 1024 * 1024` (256 MiB) | `blob.rs:19` | One blob object: `prepare_upload`, `put_stream` (declared and streamed), per workspace entry, per artifact file |
| `MAX_FIND_DIGESTS` | `512` | `blob.rs:20` | Digests per `find_missing` call (`TooManyDigests` -> 413) |
| `DEFAULT_RETENTION_MILLIS` | `30 * 24 * 60 * 60 * 1000` (30 d) | `artifact.rs:22` | Workspace, retained-manifest, and artifact horizon; applied by the caller |
| `MAX_ARTIFACT_FILES` | `4096` | `artifact.rs:23` | Files captured per execution (plus directories during recursion) |
| `MAX_ARTIFACT_TOTAL_BYTES` | `2 * 1024 * 1024 * 1024` (2 GiB) | `artifact.rs:24` | Total bytes captured per execution; decremented per file |
| `MAX_WORKSPACE_ENTRIES` | `4096` | `core:26` | Manifest entries and patch entries |
| `MAX_WORKSPACE_PATH_BYTES` | `1024 * 1024` (1 MiB) | `core:27` | Sum of all path lengths in one manifest |
| `MAX_WORKSPACE_DEPTH` | `128` | `core:28` | Path components in a manifest entry |
| `MAX_WORKSPACE_LOGICAL_BYTES` | `2 * 1024 * 1024 * 1024` (2 GiB) | `core:29` | Sum of file `size_bytes` in one manifest |
| `MAX_PATH_BYTES` | `4096` | `core:16` | One `RelativePath`, and one manifest path under the strict validator |
| `MAX_OUTPUTS` | `128` | `core:20` | Declared outputs per command (I did not verify the enforcement site at submission) |
| `MAX_CAPTURE_BYTES` | `16 * 1024 * 1024` (16 MiB) | `core:23` | **stdout/stderr** `OutputPolicy::capture_limit_bytes` only (`eggwork-runner/src/lib.rs:731,878`); it does **not** bound artifact size |
| `MAX_BLOB_FIND_REQUEST_BYTES` | `64 * 1024` (64 KiB) | `lib.rs:53` | Buffered `/v1/blobs/missing` and `/v1/blobs/prepare` bodies |
| `MAX_WORKSPACE_REQUEST_BYTES` | `4 * 1024 * 1024` (4 MiB) | `lib.rs:54` | Buffered `/v1/workspaces` and `/v1/workspaces/derive` bodies |
| GC limit | `clamp(1, 1024)` | `lib.rs:464` | Per-invocation work for every collector in `collect_garbage` |
| Reader lease | `30 * 60 * 1000` (30 min) | `lib.rs:1416`, `lib.rs:1383` | `"reader"` reference expiry, re-armed on each streamed chunk |
| Reconcile sweep | `1024` | `lib.rs:345` | `reconcile_manifests` orphan-reference limit |
| `blob_quota_bytes`, `workspace_quota_bytes` | operator-configured | `OperatorConfig` | Per-tree byte budgets, reported by `storage_summary` (`operations.rs:770-781`) |

Not verified: the default or source of `blob_quota_bytes` / `workspace_quota_bytes`; the
enforcement site for `MAX_OUTPUTS`; and any cap on retained-manifest row count (that table is
bounded only by time via `expires_unix_ms`).

## Disk layout and lifecycle

```
<blob_root>/                     0700   BlobStore::open (blob.rs:71-121)
  metadata.sqlite, -wal, -shm
  quarantine/
  .part-<uuid>/                  transient
  <xx>/<digest>                  objects

<workspace_root>/                0700   WorkspaceManager::open (workspace.rs:69-129)
  metadata.sqlite, -wal, -shm
  .staging-<uuid>/               transient
  workspace-<uuid>/              execution-private trees

<execution_root>/                         runner root for workspace-less executions
<database_path>.artifacts.sqlite  0600   ArtifactStore::open (artifact.rs:63-97)
```

Creation: `BlobStore::open` and `WorkspaceManager::open` create their roots (0700 on unix) and
metadata databases; `ArtifactStore::open` creates only the database's parent and file. Shard
directories are created lazily by `put_stream` (`blob.rs:425-426`); workspace directories appear via
the rename from staging. A materialized workspace tree *is* the execution root for a workspace-bound
execution (`lib.rs:1617-1643`), and `capture_declared` reads outputs back out of that same tree; the
blob store is the shared backing for both directions.

**After an execution:** the tree remains under `workspace-<uuid>` with its record still
`state = 'Ready'` but carrying the `expires_unix_ms` set by `mark_terminal`; artifact rows and their
blobs are present. Nothing is deleted synchronously.

**After GC:** workspace GC removes expired tree directories and rows, artifact GC removes expired
rows, blob GC removes expired references and then unreferenced blobs. Because each pass is
`LIMIT`-bounded, GC is incremental and the footprint right after GC is whatever backlog remains.

**After a crash:** each store reconciles at open, in this order in `NodeServer::start`
(`lib.rs:318-346`). `BlobStore::open` -> `remove_stale_parts` deletes `.part-` files, drops index rows
whose file is missing or whose digest no longer parses, and deletes shard files that are not indexed;
the comment at `blob.rs:573-574` names both crash windows (rename-then-metadata-commit and
metadata-commit-then-external-file-loss). `WorkspaceManager::open` -> `reconcile`
(`workspace.rs:821-863`) drops rows with an invalid `storage_key`, `remove_dir_all`s any
`.staging-*` directory and any directory not in the known storage-key set, and drops rows whose
directory is gone - so a half-written staging tree is always removed. Then
`recover_retention` / `recover_manifest_retention` re-arm every `NULL` expiry at the 30-day horizon
so a crash cannot pin a blob forever, and `reconcile_manifests(&blobs, now, 1024)` releases
`"manifest"` references whose manifest row is gone.

What is left behind: quarantined blobs (never GC'd), workspaces still inside their 30-day window,
expired artifacts not yet swept, and unreferenced blobs below the GC threshold. Node startup takes a
lock file (`<database_path>.lock`, `acquire_state_lock`, `lib.rs:540`) so two processes cannot
reconcile the same roots concurrently.

## Error model

`BlobError` (`blob.rs:33-55`) -> HTTP via `blob_error_response` (`lib.rs:1463-1486`):

| Variant | Status | Retryable | Meaning |
| --- | --- | --- | --- |
| `TooLarge` | 413 `blob_too_large` | no | Object exceeds `MAX_BLOB_BYTES` |
| `QuotaExceeded` | 507 `quota_exceeded` | yes, after GC | Byte budget exhausted |
| `LengthMismatch` | 422 `length_mismatch` | yes, re-upload | Stream ended short of `declared_length` |
| `DigestMismatch` | 422 `digest_mismatch` | yes, re-upload | Content did not hash to the declared digest |
| `CorruptExisting` | 500 `corrupt_blob` | yes, re-upload | Stored bytes failed re-verification; quarantined |
| `NotFound` | 404 `blob_not_found` | no | Not in `blobs` |
| `TooManyDigests` | 413 `too_many_digests` | no | Batch over `MAX_FIND_DIGESTS` |
| `Io`, `Metadata`, `Body`, `Worker` | 500 `storage_error` | depends | Filesystem, SQLite, stream, or lock failure |

`WorkspaceError` (`workspace.rs:20-46`) -> HTTP via the two handler arms (`lib.rs:1083-1103`,
`lib.rs:1207-1247`):

| Variant | Status | Retryable | Meaning |
| --- | --- | --- | --- |
| `InvalidManifest` | 400 `invalid_manifest` | no | Failed core validation, or a symlink entry reached `materialize_tree` |
| `InvalidPatch` | 400 `invalid_patch` | no | Patch failed core validation |
| `BaseMissing` | 409 `base_manifest_missing` | yes, with a full manifest | Derived base absent, expired, or reaped; zero side effect |
| `InvalidBlob` | 409 `missing_blob` | yes, after upload | Referenced blob missing or corrupt |
| `BlobSizeMismatch` | 409 `blob_size_mismatch` | no, as declared | Stored size differs from the manifest |
| `QuotaExceeded` | 507 `quota_exceeded` | yes, after GC | Workspace budget exhausted |
| `Conflict` | 409 `workspace_identity_conflict` | no | Same `workspace_id`, different owner/generation/manifest |
| `NotFound` | 409 `workspace_not_ready` | no | Absent, wrong owner/generation/principal, expired, or bad storage key |
| `Metadata`, `Blob`, `Io`, `Json`, `Worker` | 500 `workspace_error` | depends | Infrastructure |

`ArtifactError` (`artifact.rs:27-46`) has no dedicated HTTP mapping; callers collapse it.
`MissingRequired` (required output absent), `UnsupportedType` (symlink, device node, or other
non-file in the tree), `Limit` (`MAX_ARTIFACT_FILES` or the capture byte budget), `InvalidTree` (the
discovered tree failed manifest validation, or an `ArtifactId` could not be built), plus `Io`,
`Blob`, `Sql`, `Json`, `Worker`. `capture_declared` has no partial-success result: any variant aborts
the whole capture and the caller records `ExecutionFinalizationFailure::ArtifactCapture` while
leaving the execution's own exit state intact (`lib.rs:2459-2474`).

How a caller distinguishes the three cases:

- **Not found** - `BlobError::NotFound`; `WorkspaceError::NotFound`; for artifacts,
  `ArtifactError::MissingRequired` from `get` or an empty `list`.
- **Denied** - *not* an error type here. Authorization is decided in `lib.rs`
  (`authorize_resource` -> 403 `forbidden`), and the stores fence by principal *silently*:
  `artifact_list`/`artifact_download` filter on `principal_id`, and `resolve` returns `NotFound` for
  another principal's workspace rather than revealing that it exists (`workspace.rs:1378`). Denied
  is therefore indistinguishable from absent, by design.
- **Corrupt** - `BlobError::CorruptExisting` (quarantined), `BlobError::DigestMismatch` (inbound
  body), `WorkspaceError::InvalidBlob` (a manifest references a blob that fails re-verification),
  `WorkspaceError::BlobSizeMismatch`.

## Invariants and enforcement

| Invariant | Enforcement site | Test |
| --- | --- | --- |
| A digest names one immutable byte string | `blob.rs:408-410` (existing target -> verify) | `interrupted_upload_is_cleaned_and_duplicate_writers_deduplicate` (`blob.rs:720`) |
| Digest format is 64 lowercase hex; no traversal | `BlobDigest::parse` (`core:146`), `blob.rs:123-128` | `empty_blob_and_digest_path_are_canonical` (`blob.rs:665`) |
| Committed bytes hash to their name (ingest) | `blob.rs:451-453` | `invalid_uploads_leave_no_readable_blob_or_partial_file` (`blob.rs:685`) |
| Committed bytes still hash to their name (read) | `blob.rs:495-550` | `quota_corruption_and_startup_cleanup_are_deterministic` (`blob.rs:765`) |
| No partial upload is observable | `.part-<uuid>` at root; cleanup `blob.rs:557-613` | `blob.rs:685`, `blob.rs:765` |
| Index and filesystem agree after a crash | `blob.rs:557-613`, `workspace.rs:821-863` | `workspace.rs:1497`, `artifact.rs:856` |
| Blob bytes stay within `MAX_BLOB_BYTES` | `blob.rs:403-405`, `blob.rs:442-444` | `blob.rs:765` (`TooLarge` at `MAX_BLOB_BYTES + 1`) |
| Blob store stays within `blob_quota_bytes` | `blob.rs:411-422` (advisory: `blob.rs:165-178`) | `blob.rs:765`; `lib.rs:3399` |
| A live reference blocks GC | `blob.rs:318-324` (`NOT EXISTS` on `blob_references`) | `artifact.rs:804` |
| An in-flight download holds a reference | `lib.rs:1409-1426` + `Drop` at `lib.rs:1388` | none dedicated; indirectly `lib.rs:3399` |
| Every manifest entry is a validated portable path | `core:622` called at `workspace.rs:141-143` | `hostile_workspace_manifests_are_rejected_without_side_effect` (`lib.rs:6008`) |
| Absolute paths, `..`, `\`, depth, reserved names rejected | `core:828-878` | `lib.rs:6008`; `workspace.rs:1002` |
| No symlink is ever materialized | `core:667-671`; `workspace.rs:926` | `missing_blobs_and_invalid_symlinks_never_create_ready_workspaces` (`workspace.rs:1088`) |
| A rejected manifest creates no visible state | pre-flight before any `create_dir` (`workspace.rs:184-201`) | `lib.rs:6008`; `workspace.rs:1088` |
| A file's parent must be a declared directory | `core:673-689` | `lib.rs:6008` |
| The workspace tree is invisible until complete | staging + rename (`workspace.rs:230-292`) | `concurrent_materialization_is_idempotent_and_recovery_cleans_unready_state` (`workspace.rs:1145`) |
| Only `workspace-<uuid>` directories are roots | `valid_storage_key` (`workspace.rs:877`) in `materialize`/`resolve`/`mark_active`/GC | `workspace.rs:1002`; `workspace.rs:1430` |
| Only the owning principal may resolve | `workspace.rs:759-771`, `lookup_manifest` scoping | `cross_principal_lookup_behaves_as_missing_with_no_side_effect` (`workspace.rs:1378`) |
| Workspace bytes stay within `workspace_quota_bytes` | `workspace.rs:143-145`, `workspace.rs:171-181` | `workspace_quota_is_checked_before_materialization` (`workspace.rs:1194`) |
| A live workspace's blobs are never collected | `mark_active` -> `expires NULL` (`workspace.rs:631-660`) | `terminal_retention_gc_preserves_live_workspace_then_reclaims_inputs` (`workspace.rs:1223`) |
| Materialized bytes equal `size_bytes` | `workspace.rs:919-921` | `workspace.rs:1002` |
| A derived create equals the equivalent full create | `core:784` + shared `digest` (`core:702`) | `workspace.rs:1310`; `derived_workspace_materialization_loopback` (`lib.rs:4249`) |
| Only declared outputs are captured | `lib.rs:2445-2452`; `discover_outputs` (`artifact.rs:419`) | `captures_only_declared_files_and_streams_verified_content` (`artifact.rs:577`) |
| Artifact capture cannot follow a symlink | `open_file_beneath` `NOFOLLOW` chain (`artifact.rs:379-410`) | `descriptor_relative_capture_rejects_parent_swapped_to_symlink` (`artifact.rs:884`); `artifact.rs:663` |
| A symlinked output is refused, not copied | `artifact.rs:499-501`, `artifact.rs:471-489` | `missing_required_and_symlink_outputs_fail_without_records` (`artifact.rs:663`) |
| Capture stays within `MAX_ARTIFACT_FILES` / `_TOTAL_BYTES` | `artifact.rs:539-541`, `artifact.rs:451-453`, `artifact.rs:352-355` | `artifact.rs:577`; `disappearing_output_and_storage_quota_fail_without_publishing_records` (`artifact.rs:753`) |
| Expired artifacts are invisible, then deleted | `artifact.rs:132`, `artifact.rs:168`, `artifact.rs:186-223` | `artifact.rs:804`; `lib.rs:3399` |
| Capture is all-or-nothing w.r.t. records | retain before insert (`artifact.rs:311-322`) | `artifact.rs:753` |
| Retention cannot stay pinned after a crash | `workspace.rs:667-696`, `workspace.rs:518-533` | `workspace.rs:1430`; `workspace.rs:1497` |

## Test coverage

**`blob.rs` (4 tests).** `empty_blob_and_digest_path_are_canonical` (665) - the empty blob
`e3b0...` round-trips and its path is `root/e3/e3b0...`; `BlobDigest::parse` rejects `"../secret"`
and uppercase. `invalid_uploads_leave_no_readable_blob_or_partial_file` (685) - a digest mismatch and
a length mismatch both fail with **no** readable blob and **no** leftover `.part-`.
`interrupted_upload_is_cleaned_and_duplicate_writers_deduplicate` (720) - a failing body stream
leaves nothing behind and 12 concurrent writers of one digest converge on a single file.
`quota_corruption_and_startup_cleanup_are_deterministic` (765) - `TooLarge` at
`MAX_BLOB_BYTES + 1`, `QuotaExceeded` at a zero quota, a deliberately corrupted object quarantined
and its row deleted, `.part-` files swept at reopen, and `find_missing` reporting the removed digest
as missing afterwards. That one test covers digest mismatch, quota exhaustion, corruption/quarantine,
and startup cleanup.

**`workspace.rs` (10 tests).** Exact tree materialization with 0600/0700 permissions and fenced
resolution (1002); missing blobs and symlink manifests never producing a ready workspace (1088);
concurrent same-identity materialization plus recovery of unready state (1145); quota checked
*before* materialization (1194); retention GC preserving a live workspace and later reclaiming its
inputs (1223); full create registering a base and a same-principal derived create matching the full
manifest digest (1310); cross-principal lookup behaving as missing with no side effect (1378); an
expired base behaving as missing and GC releasing the blobs it pinned (1430); a manifest whose blob
is missing being reaped with crash/reopen reconciliation (1497); and 12-way concurrent derived
materialization with conflict fencing and invalid patches (1543).

**`artifact.rs` (6 tests).** Only declared files captured with content streaming back verified (577);
missing required and symlinked outputs failing with no records published (663); a disappearing
output and a zero-quota store both failing without publishing records (753); retention references
protecting shared blobs with bounded, restart-safe GC (804); startup reconciling a crash after blob
unlink during GC (856); and the unix-only
`descriptor_relative_capture_rejects_parent_swapped_to_symlink` (884).

**Server integration (`lib.rs`).**
`blob_protocol_streams_verifies_deduplicates_and_enforces_quota` (3399) is the broad one: a 2 MiB
blob uploads and downloads, `find_missing` becomes empty, a **same-digest upload is a verified
no-op even when the quota is full**, a second blob hits 507, a wrong digest hits 422; then it
creates a workspace, runs an execution in it, asserts `artifact_count == 1`, lists the artifact,
downloads it and compares bytes, and finally asserts a *failed finalization* reports
`artifact_count == 0` with `ExecutionFinalizationFailure::ArtifactCapture`.
`derived_workspace_materialization_loopback` (4249) covers capability advertisement, the
full -> derived flow, equality of locally reconstructed digests, full/derived convergence, retry
idempotency, unknown-base and cross-principal `base_manifest_missing`, and malformed-patch rejection.
`derived_workspace_denied_and_oversized_bodies_are_typed` (4499) covers 403 and the 413 body bound.
`hostile_workspace_manifests_are_rejected_without_side_effect` (6008) is the traversal-rejection test,
and `required_landlock_is_admitted_remotely_and_denies_outside_workspace_access` (4699) covers the
runner's confinement to the materialized root.

**Honest gaps.**

- No test asserts that a blob cannot be unlinked by GC while `blob_download` streams - the `reader`
  lease is not used there at all, so that path is both unprotected and untested.
- No test drives a manifest that `RelativePath` accepts but the strict portable validator rejects (a
  Windows-reserved name, a trailing dot, a non-ASCII byte) through the `workspace_create` handler.
- No test exercises the non-unix branches: `open_file_beneath` returning `ErrorKind::Unsupported`,
  the `#[cfg(not(unix))]` permission helpers, or the `sync_directory` no-op.
- No test covers `reconcile_manifests`' orphan-reference sweep independently of a crash, nor a
  hardlink-based quota bypass, nor unbounded `retained_manifests` growth.
- Nothing in `crates/eggwork-server/tests/` touches the data plane: `installed_qualification.rs` and
  `release_contract.rs` are the only integration files.
- Recorded evidence: [001](../plans/closure/workspace-artifact-transport/001-status.md) through
  [004](../plans/closure/workspace-artifact-transport/004-status.md); 004 records
  `cargo test --workspace --all-targets` as 108 passed, 1 ignored, 8 suites, and enumerates the
  manifest-CAS tests added last.

## Review focus

- **TOCTOU between validation and file creation in `materialize_tree`** (`workspace.rs:887-939`).
  The destination is opened with `create_new(true)` on a plain `Path::join`, with no `O_NOFOLLOW` and
  no descriptor-relative walk - unlike artifact capture, which uses `openat`/`NOFOLLOW`. The
  mitigation is that the staging root is fresh, 0700, and contains only this module's own
  `create_dir` results. Confirm the threat model holds, and whether `openat2(RESOLVE_BENEATH)` is
  worth the cost.
- **GC racing a concurrent `retain`.** Blob GC holds `write_lock` and an `IMMEDIATE` transaction, but
  `retain` takes only the metadata mutex. A `retain` committing between `fs::remove_file` and
  `DELETE ... AND NOT EXISTS (...)` leaves a live reference pointing at a deleted file; the guard
  correctly keeps the row and `remove_stale_parts` cleans the dangling row at the next open, but the
  window is real and untested.
- **Asymmetric download protection.** `artifact_download` takes a `"reader"` lease;
  `blob_download` does not. Verify the intended invariant is "unreferenced bytes may vanish
  mid-transfer, but the open fd keeps working on unix", and note that elsewhere the removal would
  fail and abort the whole GC transaction.
- **A leaked or stalled `reader` reference is bounded but invisible.** `Drop` is the only thing
  between a dropped body and a retained blob, and `renew()` is called only on a successful chunk
  read; a download that stalls past 30 minutes loses its protection mid-stream.
- **Over-long upload bodies report as `TooLarge`, not `LengthMismatch`** (`blob.rs:442-444`). A
  client that streams more than it declared gets 413 and may conclude the object is too big to
  upload, when the correct action is to re-declare. Check the client handles this.
- **Quota accounting blind spots.** `SUM(size_bytes)` covers only committed, indexed objects:
  quarantine, `.part-`, and unindexed strays are uncounted, and a corrupt-then-re-uploaded object
  burns quota twice. `prepare_upload` is advisory and lock-free, so the authoritative check is the
  one in `put_stream` - a client that skips `prepare` still gets a correct 507.
- **Quota bypass via duplicate content and hardlinks.** One large blob can back many manifests
  while the workspace quota counts `logical_bytes` per workspace, so N workspaces over one blob count
  as N times the bytes. The module has no hardlink detection - `blob_references` rows are logical,
  not physical link counts - so a filesystem-level hardlink into the blob root is undetectable. The
  per-object bound, the quota, and root ownership are the only limits.
- **Unbounded in-memory manifest handling.** `MAX_WORKSPACE_ENTRIES` and `MAX_WORKSPACE_PATH_BYTES`
  cap the decoded manifest and `MAX_WORKSPACE_REQUEST_BYTES` caps the body, but `retained_manifests`
  is bounded only by time. Confirm one principal cannot grow it without limit within 30 days.
- **Cleanup following symlinks.** Workspace GC does `fs::remove_dir_all(root.join(key))` after
  `valid_storage_key` (`workspace.rs:734-742`), a plain recursive removal. Since `materialize_tree`
  never creates symlinks the tree cannot contain one, but any other write path into the workspace
  root could introduce one. Artifact capture refuses to follow symlinks; workspace teardown is
  descriptor-relative in no way and is the weaker of the two. Blob GC uses `remove_file` on a leaf,
  so it is unaffected.
- **`reconcile` deletes any directory it does not recognize** (`workspace.rs:843-851`), including a
  `.staging-` directory belonging to a live concurrent materialization. Today `reconcile` runs only
  from `WorkspaceManager::open`, and the state lock plus startup ordering prevent that; the safety
  argument lives entirely there.
- **`retain` requires the digest to exist but does not re-verify** (`blob.rs:199-232`). Reference
  creation is a metadata operation; integrity comes from the caller's preceding `verify_existing`, so
  any future caller that retains without verifying can pin a corrupt blob.
- **Whether any traversal defense depends on the caller having validated first.** The manifest path
  comes off the wire and is validated by core before use; the *execution root* passed to
  `capture_declared` is derived from `ReadyWorkspace.root`, itself re-derived from `storage_key`
  under `valid_storage_key`; `DeclaredOutput.path` is only `RelativePath`-validated, with the strict
  rules arriving one step later via the preflight `WorkspaceManifest::validate`. The code is
  correct, but the guarantee for declared outputs sits one step from where a reader expects it.
- **Artifact GC does not release blob references** (`artifact.rs:186-223`), so one artifact's bytes
  need both collectors, in the right order - something `NodeServer::collect_garbage` guarantees only
  by call sequence. `deleted_artifacts` is a bounded candidate count, not a measured row count.
- **`insert_many` is effectively never idempotent.** The
  `UNIQUE(execution_id, generation, principal_id, record_json)` constraint exists, but each capture
  generates a fresh `ArtifactId` UUID, so it can almost never fire: a repeated capture creates a
  second record set for the same output. If finalization can run twice, the artifact list duplicates.

## Related

- [overview.md](overview.md) - system-level orientation.
- [core-domain.md](core-domain.md) - `BlobDigest`, `RelativePath`, `WorkspaceManifest`,
  `ArtifactRecord`, and the bound constants in `eggwork-core`.
- [server-node.md](server-node.md) - the node process that owns these three stores.
- [storage-journal.md](storage-journal.md) - the execution journal and its relationship to the
  artifact store's own database.
- [operations-cli.md](operations-cli.md) - `collect_garbage`, `storage_summary`, `inspect_blob`,
  `inspect_workspace`, `inspect_artifact`, and the quota configuration surface.
- [runner-execution.md](runner-execution.md) - how `ReadyWorkspace.root` becomes the runner's
  execution root and what confinement applies to it.
- [ADR-0004: Content-Addressed Portable Workspaces and Declared
  Artifacts](../plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md) - the normative
  decision.
- Plans: [001 blob store and digest
  protocol](../plans/implementation/workspace-artifact-transport/001-blob-store-and-digest-protocol.md),
  [002 workspace manifest and safe
  materialization](../plans/implementation/workspace-artifact-transport/002-workspace-manifest-and-safe-materialization.md),
  [003 declared artifacts retention and
  GC](../plans/implementation/workspace-artifact-transport/003-declared-artifacts-retention-and-gc.md),
  [004 reusable manifest CAS and derived
  materialization](../plans/implementation/workspace-artifact-transport/004-reusable-manifest-cas-and-derived-materialization.md).
- Recorded evidence: [001](../plans/closure/workspace-artifact-transport/001-status.md),
  [002](../plans/closure/workspace-artifact-transport/002-status.md),
  [003](../plans/closure/workspace-artifact-transport/003-status.md),
  [004](../plans/closure/workspace-artifact-transport/004-status.md).
