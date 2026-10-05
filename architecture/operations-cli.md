# operations CLI

`eggworkd` is the JSON-only local operator interface to an Eggwork node. It reads a
versioned JSON configuration from the local filesystem, reports bounded read-only
inspections of local state, and drives the host platform's service manager and the
Eggup release transaction. Every verb that produces a result prints a single JSON
document to stdout, and every failure prints a fixed human string to stderr with exit
code 2, so a script never has to parse prose to learn what happened. The library half
of this surface lives in `crates/eggwork-server/src/operations.rs`; the binary half is
`crates/eggwork-server/src/bin/eggworkd.rs`.

## Responsibility boundary

**Owns.**

- Parsing a local operator configuration document into `OperatorConfig` and refusing it
  on any trust, shape, or bound violation.
- Turning that configuration into the node's runtime identity: mTLS server policy, the
  principal resolver, and the grant authorizer.
- Bounded, mostly read-only inspection: execution listings, single-execution reads, storage
  accounting, metric counters, and per-resource inspection of blobs, workspaces, artifacts.
- The persistent drain marker — the single bit of node state the operator controls
  out-of-band.
- Operator-initiated garbage collection, with a read-only preview as the default.
- Host service lifecycle and Eggup release application, by delegating to Eggup adapters
  rather than reimplementing a service manager.

**Does not own.**

- **It is not a remote administration API.** There is no operator listener, no operator
  authentication, and no remote operator protocol. The `eggserv` control plane is
  described in [server-node.md](server-node.md); `eggworkd` is a local process on the node
  host.
- It does not schedule, place, retry, or dispatch executions. Those belong to the control
  plane and the node's request path ([data-plane.md](data-plane.md),
  [core-domain.md](core-domain.md)).
- It does not authenticate *to* the node. It opens the execution database read-only and
  reads local filesystem trees; it never speaks the client transport
  ([client-transport.md](client-transport.md)).
- It does not own release packaging, signing, or publishing. `deployment apply` accepts
  candidates that are already on local disk ([distribution.md](distribution.md),
  [release-qualification.md](release-qualification.md)).
- It does not re-implement the runner's trust policy. Helper trust is delegated to
  `eggwork_runner::verify_trusted_helper` ([sandbox-helper.md](sandbox-helper.md),
  [runner-execution.md](runner-execution.md)).

## Operator configuration

`OperatorConfig` is `#[serde(deny_unknown_fields)]`, so an unrecognized key is a hard
load error rather than a silently ignored setting.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `schema_version` | `u16` | yes | Must be exactly `1`. |
| `node_id` | `String` | yes | Parsed through `NodeId::new`. |
| `bind` | `String` | yes | Parsed through `SocketAddr`. |
| `execution_root` | `PathBuf` | yes | Relative paths resolve against the config's parent directory. Must exist as a trusted directory. |
| `database_path` | `PathBuf` | yes | The execution database. May not exist yet; its parent must be a trusted directory. |
| `blob_root` | `PathBuf` | yes | Must exist as a trusted directory. |
| `blob_quota_bytes` | `u64` | yes | Must be nonzero. |
| `workspace_root` | `PathBuf` | yes | Must exist as a trusted directory. |
| `workspace_quota_bytes` | `u64` | yes | Must be nonzero. |
| `max_active_executions` | `u32` | yes | `1..=65_536`. |
| `lease_ttl_seconds` | `u64` | yes | `1..=86_400`. |
| `sandbox_helper` | `Option<PathBuf>` | no (the only optional field) | `None` means no helper, which fails isolation closed. |
| `tls` | `TlsFiles` | yes | See below. |
| `clients` | `Vec<ClientGrant>` | yes | Must be `1..=1024` entries. |

`TlsFiles` is also `deny_unknown_fields` and carries exactly three paths:
`certificate_chain`, `private_key`, and `client_ca`. All three, plus every other path
field, are resolved relative to the config file's own directory when they are relative —
so a config can be moved as a unit.

The shape in `examples/eggworkd.example.json` matches this table exactly, with one
`clients` entry granting all eleven operation names and absolute paths under
`/var/lib/eggwork` and `/etc/eggwork`.

**Load-time checks (`OperatorConfig::load`).** Before any parsing, `fs::symlink_metadata`
must show a regular file that is not a symlink, is at most `MAX_CONFIG_BYTES` (1 MiB), and
is not writable by others. The read itself is capped with `.take(MAX_CONFIG_BYTES + 1)`, so
a file that grew after the metadata read still cannot be slurped without bound.

**Validation-time checks (`OperatorConfig::validate`).** Beyond the field bounds above:
every `certificate_sha256` must be exactly 64 hex characters, fingerprints must be unique
across the grant set (compared lowercased), every named operation must resolve, and
`tls_config()` must build successfully — which reads the chain and the client CA through
`read_certificates` (regular file, not a symlink, not writable by others, non-empty) and
the private key through `trusted_private_key`.

The directory and file trust predicates, on Unix:

- `trusted_directory`: owned by the effective uid, `mode & 0o700 == 0o700`, and
  `mode & 0o022 == 0`.
- `config_writable_by_others`: owned by someone other than root or the effective uid, or
  writable by group/other.
- `owned_writable_file`: owned by the effective uid with `mode & 0o200 != 0`.
- `private_key_readable_by_others`: `mode & 0o077 != 0`.

On non-Unix targets these predicates are the trivial `true`/`false` stubs; the Unix
ownership and mode checks do not exist there.

**Redaction.** `OperatorConfig::redacted_json` serializes the whole config and then
replaces exactly two things: `tls.private_key` and every `clients[].certificate_sha256`,
each with the literal string `[REDACTED]`. Only `config print` uses this. Why it is a
security property and not cosmetics: `config print` output is the artifact an operator
pastes into a ticket, a chat channel, or a support bundle. The private key *path* names
where the key lives, and the client fingerprints bind a principal to a leaf certificate —
together they are the two fields an attacker needs to start targeting key custody or to
fingerprint a specific client. Everything else printed (chain path, client CA path,
principal ids, quotas) is an identifier the operator already needs. Note precisely what is
redacted: the *strings* are replaced, and no key material is ever loaded into the redacted
document to begin with.

## Authorization model

A `ClientGrant` is a principal plus an explicit set of operation names:
`principal_id`, `certificate_sha256` (documented in source as the lowercase hex SHA-256
digest of the verified leaf certificate DER), and `operations: Vec<String>`.

`OperatorConfig::principals()` builds a pair. Every grant contributes
`(certificate_sha256.to_ascii_lowercase(), PrincipalId)` to a
`FingerprintPrincipalResolver`, and a `HashSet<Operation>` to a `GrantAuthorizer`. The
lowercasing on both sides is what makes config-time and handshake-time fingerprints
comparable.

`GrantAuthorizer` is a newtype over `HashMap<PrincipalId, HashSet<Operation>>` and
implements the server's `Authorizer` trait with a single method:

```rust
fn authorize(&self, principal: &NodePrincipal, operation: Operation) -> bool {
    self.0.get(&principal.id).is_some_and(|allowed| allowed.contains(&operation))
}
```

The default is fail-closed by construction: an unknown principal produces `None` from the
map lookup, an ungranted operation is absent from the set, and both collapse to `false`
through `is_some_and`. There is no allow-by-default branch anywhere in the file.

The name → enum mapping is a closed 11-name table in `fn operation`: `capabilities`,
`status`, `execute`, `observe`, `cancel`, `renew`, `events`, `blob-read`, `blob-write`,
`workspace-create`, `artifact-read`. Anything else returns `None`, which `validate` treats
as a config error, so a typo in a grant is a startup refusal rather than a silently
missing permission.

**Scope of the policy, stated conservatively.** In `operations.rs` the authorizer
overrides *only* `authorize`. This file does not implement, and does not reference, any
resource-aware `authorize_request` hook. I could not verify from the two files owned by
this deep dive whether the `Authorizer` trait in `lib.rs` declares such a hook and what
its default body does; the resolution is recorded in [server-node.md](server-node.md). The
operationally important consequence is the part that *is* visible here: the policy
expressed in this file is a principal × operation product with no resource dimension at
all. Any per-resource narrowing a deployment needs is therefore not produced by
`GrantAuthorizer` and must be enforced by the request path.

## Command surface

`run()` requires `--config <path>` for every verb except `version`, locates the *first*
occurrence of the literal `--config` in argv, takes the next argv token as the path, and
removes both from the argument vector. The remaining vector keeps the subcommand at index
0, so `args[1]` is the first sub-verb and `args[2]` its first operand.

| Command | Required flags | Output |
| --- | --- | --- |
| `version` | none (the only verb that runs before `--config` is located) | `{"version": <CARGO_PKG_VERSION>}` |
| `config validate` | `--config` | `{"valid": bool, "doctor": DoctorReport}`; exit 2 when `valid` is false |
| `config print` | `--config` | The redacted configuration document, pretty-printed |
| `doctor` | `--config` | `DoctorReport`; exit 2 when `report.ready` is false |
| `run` | `--config` | `{"ready": true, "node_id": ..., "local_addr": ...}`, then blocks on `ctrl_c` and shuts the server down |
| `status` | `--config` | `schema_version`, `node_id`, `draining`, `active_executions`, `max_active_executions`, `execution_records`, `execution_states`, `stdout_bytes`, `stderr_bytes`, `cleanup_failures`, `metrics`, `doctor` |
| `drain` | `--config` | `{"draining": true, "persistent": true}` |
| `undrain` | `--config` | `{"draining": false, "persistent": true}` |
| `executions list` | `--config`, optional `--limit N` (default 50, clamped 1..=200), `--offset N` (default 0, capped at 2048) | `ExecutionPage` |
| `executions show` | `--config`, positional `<execution-id>`, optional `--generation N` | One `ExecutionSnapshot` |
| `storage summary` | `--config` | `StorageSummary` |
| `inspect blob <digest>` | `--config`, positional digest | `ResourceInspection` |
| `inspect workspace <id>` | `--config`, positional id | `ResourceInspection` |
| `inspect artifact <id>` | `--config`, positional id | `ResourceInspection` |
| `gc` | `--config`, optional `--apply`, `--limit N` (default 128, clamped 1..=1024) | `NodeGcReport` with `dry_run` set accordingly |
| `deployment status` | `--config` | `schema_version`, `deployment`, `service_id`, `executable`, `platform` |
| `deployment apply` | `--config`, `--installation-root`, `--release`, `--daemon`, `--service-config` (+ platform conditions, see below) | Release transaction JSON report |
| `service spec` | `--config`, `--service-config <abs>`; `--executable <abs>` or a canonicalizable `current_exe` | `service_id`, `executable`, `args`, `config` |
| `service status` | as above + platform policy flags | `service_id`, `platform`, `backend`, `ownership`, `state` |
| `service install` / `start` / `stop` / `restart` / `uninstall` | as above + platform policy flags | `service_id`, `platform`, `backend`, `operation`, `completed` |
| unknown verb | — | usage string on stderr, exit 2 |

**Flags that exist in `usage()`'s prose but are worth checking.** The usage string's
closing clause — "service backends require explicit systemd, launchd, or Windows SCM
policy" — is *not* a flag. There is no `--backend`, `--platform`, or `--policy` switch
anywhere in the file: the backend is chosen by compile-time `cfg!` inside
`product_service_manager` and named by `service_backend()`. The same usage line does say
"deployment apply accepts local `--daemon`/`--helper` candidates", which is accurate.

**Corrections to a naive reading of the command list.** `config print` is fully
implemented and appears in `usage()`, even though it is easy to overlook next to
`config validate`. Conversely, there is **no** `--help` or `-h` handler anywhere in the
binary: `eggworkd --help` is parsed as command `--help`, fails the `--config` lookup, and
returns the usage string on stderr with exit 2. There is also no `--quiet`, no `--json`,
and no `--output` switch; JSON is not a mode, it is the only mode.

**Argument handling is permissive in one specific way.** `parse_option` and
`option_value` find a flag by first-occurrence position scan and take the next token. No
verb validates arity, so trailing junk after a complete invocation is silently ignored —
`eggworkd gc --apply --config C nonsense` performs an apply and exits 0. A duplicate
`--limit` silently uses the first occurrence.

## Output and exit-code discipline

There is one output helper, `output(&impl Serialize)`, which prints
`serde_json::to_string_pretty`. Every command result goes through it except two: `config
print`, which prints `redacted_json()` (also pretty JSON, but a different call path), and
`run`, which prints a compact `serde_json::json!` object for its readiness line. Both
still emit a single JSON document, but the formatting differs, so a consumer should parse
rather than diff lines.

`main` installs the rustls ring provider as the process default before doing anything —
the libraries install no global provider, and without this every TLS config reports
invalid, which would make `config validate` and `doctor` fail for the wrong reason. It
then maps `Ok(())` to `ExitCode::SUCCESS` and any `Err` to `eprintln!("eggworkd: {error}")`
plus `ExitCode::from(2)`.

**Exit codes are binary, and that matters.** There is exactly one failure code. `doctor`
and `config validate` print their complete, valid JSON report to stdout *and then* return
`Err`, so exit code 2 does not mean "no output was produced" — it means "output was
produced and it described a problem". A partial failure is not separately represented:
a `deployment apply` that rolled back and one that committed and failed a post-commit
check both exit 2, and the difference is only in the JSON body.

**Error text is a fixed vocabulary.** The library's `OperationsError` renders to strings
like "operator configuration is invalid", and the binary discards them entirely
(`map_err(|_| "configuration is invalid or unreadable")`). Two consequences follow. The
first is that a bad config path never reveals *why* — the underlying `Io` error, including
its path and errno, is dropped. The second is that this is also a redaction boundary:
because the binary never interpolates an error's source text for library errors, no
filesystem path, key detail, or child-process output can reach stderr through that path.

**Redaction on every path.** The only command that emits configuration content is
`config print`, and it emits only the redacted form. `config validate` emits the doctor
report, not the config. `status` emits `node_id` and counters, not config. The
`deployment apply` report is explicitly bounded — the source comment notes that failure
`detail` is already bounded by Eggup's failure report, so "this never emits unbounded
child output" — and the post-install version probe is run through `eggup_core::run_bounded`
with `MAX_VERSION_PROBE_BYTES` and a timeout, so a hostile or wedged installed daemon
cannot flood the operator's terminal. Errors from the Eggup service and deployment layers
*are* propagated with `error.to_string()`, so those messages are trusted to be
non-secret; that trust is an assumption, not something enforced in this file.

## `doctor` and `start_server`

`DoctorReport` is `schema_version: 1`, `ready: bool`, and `checks: Vec<DoctorCheck>`;
`DoctorCheck` is a `&'static str` name, an `ok` bool, and a free-text `detail`. `ready` is
simply `checks.iter().all(|check| check.ok)`. Seven checks, in order:

1. **`configuration`** — `config.validate().is_ok()`. Its detail distinguishes "valid" from
   "configuration or TLS material is invalid", but not which field.
2. **`sandbox-resource-backend`** — the helper check. `trusted_helper` delegates to
   `eggwork_runner::verify_trusted_helper(path).is_ok()`. When a helper is configured and
   trusted, the runner is constructed as `LocalProcessRunner::new(TrustedLandlockSetup::new(helper))`
   and `execution_capabilities()` is awaited, and the detail lists the runtime-probed
   capabilities. A trusted helper that passed ownership checks but advertises nothing
   reports ok with "no execution capability passed runtime probes".
3. **`listener`** — `TcpListener::bind` succeeds, *or* a `TcpStream::connect_timeout` with
   a 150 ms timeout succeeds. The second disjunct is what makes the check meaningful while
   the node is already running: a bound port is a healthy port here.
4. **`execution-root`**, 5. **`blob-root`**, 6. **`workspace-root`** — `fs::metadata` shows
   a directory and `read_dir` succeeds.
7. **`storage-headroom`** — `fs2::available_space(&execution_root)` is `Some(n)` with
   `n > 0`.

Two honest notes on these checks. The root checks use `fs::metadata`, which follows
symlinks, whereas `validate` uses `fs::symlink_metadata` and rejects symlinked roots — so
`doctor` can report a root as available where `validate` would reject the same path. And
check 1 is the only place TLS policy is exercised, because `validate` calls `tls_config()`.

**The invariant that makes `doctor` worth running.** The source comment on
`trusted_helper` is the whole argument: it deliberately delegates instead of restating the
policy, because a stricter local copy would deny a helper the runner would accept, turning
every unprivileged user-scope installation into a node that can never advertise required
filesystem isolation, and — more importantly — would make `doctor` *contradict the
enforcement path it is meant to describe*. Both `doctor` and the node's execution path call
the same `verify_trusted_helper`, so the answer `doctor` prints about helper trust is by
construction the answer the enforcement path would give. Helper install and trust details
are in [sandbox-helper.md](sandbox-helper.md).

**`start_server`** assembles the node from the config and nothing else:
`config.node_config()` produces the `NodeConfig` (including `lease_ttl:
Duration::from_secs(lease_ttl_seconds)`), `config.principals()` produces the
`(FingerprintPrincipalResolver, GrantAuthorizer)` pair, and the runner is
`LocalProcessRunner::new(TrustedLandlockSetup::new(helper))` when a helper is configured
or `LocalProcessRunner::new(NoExecutionSetup)` when it is not. `NodeServer::start` is
awaited and every error is collapsed to `OperationsError::Config`, so the binary can only
report "node failed to start; check doctor and service logs". The TLS policy is the one
built by `tls_config()`: a single identity from the configured chain and key, and
`client_auth_required(roots)` from `client_ca` — mutual TLS is not optional, there is no
branch that builds a server-only configuration. The client CA is therefore required even
for a node that is only ever driven locally. Drain is *not* consulted by `start_server`
itself; the marker is observed by the node's request path, which is
[server-node.md](server-node.md)'s subject. Transaction and lifecycle details around
starting a node under a service manager are in [deployment-lifecycle.md](deployment-lifecycle.md).

## Drain

`OperatorConfig::drain_path()` is `self.database_path.with_extension("drain")` — a marker
file beside the execution database, sharing its base name and directory. (Verified from
source; the example config's `database_path` of `/var/lib/eggwork/state/executions.sqlite`
yields `/var/lib/eggwork/state/executions.drain`.)

`set_persistent_drain(path, true)` is a crash-safe create: `create_dir_all` the parent,
write to a unique `path.with_extension("drain.<uuid>.tmp")` opened `write` +
`create_new` with mode `0o600` on Unix, write the literal `b"draining\n"`, `sync_all`, then
`fs::rename` over the target, then `fs::sync_all` the parent directory. The uniqueness
plus `create_new` means a colliding temp name is an error rather than a clobber.
`set_persistent_drain(path, false)` removes the file, treating `NotFound` as success (so
`undrain` is idempotent), and fsyncs the parent afterwards.

`is_persistently_draining(path)` is a single `fs::symlink_metadata` check for "is a
regular file". It does not read the contents, does not follow symlinks, and a directory
named `*.drain` reads as *not* draining.

**Who observes it.** The node's request path, on every request, reads the marker and
refuses new execution admission while it is set — the `rejected_draining` counter in
`METRIC_NAMES` is the observable trace. `eggworkd status` reports the same bit as
`draining`. `deployment apply` passes the marker to the release transaction as
`drain_marker: Some(&drain_marker)`.

**Drain-first-then-replace.** The CLI's contribution to that ordering is passing
`drain_marker: Some(..)` into `UpdateRequest`. The sibling deep dive records the mechanics:
[deployment-lifecycle.md](deployment-lifecycle.md) states that when `drain_marker` is
`Some` the transaction sets the marker via `operations::set_persistent_drain(marker, true)`
first and that **drain is set before replacement**, with quiescence enforced through
`wait_for_quiescence`. The `active_execution_count` closure passed as the quiescence probe
is the fail-closed primitive described below. `--force` sets
`UpdatePolicy { force: true, ..default }`, which is the documented way to skip that wait.

**Persistent drain survives success.** `deployment apply`'s report hard-codes
`"drain_remains_active": true`, and the CLI never clears the marker on any path. This is
deliberate and asymmetric: after a release, the operator must confirm the node is healthy
and then run `eggworkd undrain --config ...` themselves. The rationale is that the
person who ran the upgrade is the only one who knows whether the new binary is behaving,
and an automated tool silently re-admitting executions would let a bad release take
traffic. The cost is real and worth stating: an operator who forgets `undrain` leaves a
node that looks healthy on `doctor` and rejects every execution with
`rejected_draining`. `status` and the drain assertion in the release harness exist to make
that state visible.

## Inspection and reporting

`execution_page(config, limit, offset)` is the workhorse. It clamps `limit` to
`1..=MAX_EXECUTION_PAGE` (200) and `offset` to 2048, requires the database to be a
non-symlink regular file, then loads *all* durable snapshots via `load_snapshots` — which
caps its query at `LIMIT 2049` and returns `OperationsError::Bounds` above 2048 rows.
That is the honest bound: **this is not a database-level page.** The whole bounded set is
decoded, then `total`, `active_executions` (Accepted, Preparing, Running, Cancelling), a
`BTreeMap` of state labels, `stdout_bytes`, `stderr_bytes`, and `cleanup_failures` are
aggregated over the entire set; only then is the vector sorted by
`(execution_id, generation)`, offset-drained, and truncated to `limit`, with `next_offset`
`Some` only when `offset + returned < total`. The reported `states`, byte totals, and
`total` describe the whole database, not the page. Byte accumulation is `saturating_add`.

`ExecutionPage` is: `limit`, `offset`, `total`, `active_executions`, `states`,
`stdout_bytes`, `stderr_bytes`, `cleanup_failures`, `next_offset`, `executions`.

`execution_show(config, id, generation)` is the narrow, targeted read: validate
`ExecutionId`, optionally wrap `generation` in `ExecutionGeneration`, require a regular
database file, open it read-only, and run one of two queries — a two-column key lookup
when a generation is given, or `ORDER BY generation DESC LIMIT 1` when it is not. The CLI
maps `--generation 0` to `None` (via `(generation != 0).then_some(...)`), so `0` is the
"latest" sentinel rather than a real generation.

`active_execution_count` is the quiescence probe, and its failure behavior is the point:
a missing database yields `0`; a database that is not a regular non-symlink file yields
`usize::MAX`; any load or decode error yields `usize::MAX`. Both non-zero answers mean
"never quiet", so an unreadable database stalls a release rather than authorizing one. The
source comment says so directly.

`storage_summary` is the one genuinely *unbounded* operation in this file: `tree_bytes`
walks `blob_root` and `workspace_root` iteratively with no limit, summing file lengths and
skipping symlinks (it checks `file_type().is_symlink()` and `continue`s, so a symlink is
neither followed nor counted). Execution and artifact sizes instead use `file_family_bytes`,
which sums the main file plus `-wal` and `-shm` siblings — so the reported execution
footprint includes un-checkpointed WAL. The artifact database is
`database_path.with_extension("artifacts.sqlite")`. `filesystem_free_bytes` is `None` when
the measurement fails, and quotas are echoed so the operator computes utilisation — the
CLI does not.

`metrics_snapshot` pre-seeds every name in `METRIC_NAMES` with `0` and then overlays
values from the `node_metrics` table. Three behaviors matter: a missing database returns
all zeros; a *missing table* also returns all zeros rather than failing, so a
pre-metrics database reports a clean zeroed snapshot; and negative stored values are
clamped to `0`. Only names present in `METRIC_NAMES` are ever surfaced — the overlay does
`if let Some(counter) = counters.get_mut(&name)`, so an unlisted row is silently ignored.
`METRIC_NAMES` has 20 entries: `executions_accepted`, `rejected_route`,
`rejected_unauthenticated`, `rejected_unauthorized`, `rejected_invalid`,
`rejected_draining`, `rejected_capability`, `rejected_busy`, `rejected_storage`,
`event_history_resync`, `blob_upload_bytes`, `blob_download_bytes`, `stdout_bytes`,
`stderr_bytes`, `terminal_succeeded`, `terminal_failed`, `terminal_cancelled`,
`terminal_timed_out`, `terminal_interrupted`, `cleanup_failures`. The store can write
counters beyond this list; those are not reported here. [storage-journal.md](storage-journal.md)
covers the schema and the write side, and a test asserts the set is fixed and bounded.

All read paths use `read_only_database`, which opens with
`SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX` — the CLI never writes to the execution or
artifact databases. Only `drain` and the GC lock file are written, and only by explicit
operator verbs.

The three `inspect_*` functions each return `ResourceInspection { kind, id, metadata }`
where `kind` is a `&'static str` and `metadata` is a free-form `serde_json::Value`:

- `inspect_blob` parses a `BlobDigest`, resolves the two-hex-character shard directory,
  rejects a symlinked shard or a symlinked blob file, reads the blob's `size_bytes` and
  `reference_count` from the blob store's `metadata.sqlite`, and **cross-checks the
  recorded size against the file's actual length**, reporting `OperationsError::Data` on
  mismatch. It is therefore a corruption check, not just a lookup. Reveals size, reference
  count, and stored-ness. Bounded to one shard entry.
- `inspect_workspace` reads one row: `state`, `logical_bytes`, `manifest_digest`,
  `expires_unix_ms`. Single indexed query.
- `inspect_artifact` reads `record_json` and deserializes the full
  `eggwork_core::ArtifactRecord`, re-serializing it as `metadata`. It reveals whatever the
  artifact record contains, which is the whole record rather than a projection.

All three read only, and all three map a missing or unreadable resource to
`OperationsError::NotFound`, which the binary flattens to "resource was not found or could
not be inspected" — indistinguishable from the outside.

## Garbage collection

`collect_garbage(config, dry_run, requested_limit)` clamps the limit to
`1..=MAX_GC_BATCH` (1024) and then splits.

**The default is a read-only preview.** Without `--apply`, `dry_run` is true and
`dry_run_gc` runs a fixed set of read-only aggregate queries: expired workspaces with a
count and summed `logical_bytes`; expired retained manifests; expired artifacts; expired
blob *references*; and unreferenced blobs (`reference_count = 0`, ordered by
`last_used_unix_ms`) with a count and summed `size_bytes`. All except the manifest query
are bounded by `LIMIT ?`; the manifest query is wrapped in `unwrap_or(0)` with an explicit
comment that older stores predate the retained-manifest cache and the preview should
report zero rather than fail. All counts are `.max(0) as u64`, and every `*_deleted` field
is `0`. **What the limit implies:** the preview is a bounded *sample* of the oldest
eligible entries, not an exhaustive census. A nonzero count proves there is garbage; a
zero count does not prove there is none when the eligible set exceeds the limit.

**Applying takes an exclusive lock, which is why it cannot run against a live node.**
`acquire_maintenance_lock` opens `database_path.with_extension("lock")` (mode `0o600`)
and calls `fs2`'s `try_lock_exclusive` — a non-blocking attempt that maps failure to
`OperationsError::Data`. The running node holds that same lock, so `gc --apply` fails
while the node process is up. That is the intended design: applying opens
`WorkspaceManager`, `BlobStore`, and `ArtifactStore` in *writable* mode and mutates them,
so overlapping a live node's writes would be a data race. The CLI's error text is the
generic "bounded garbage collection failed", which is unfortunate but not wrong. The
contrast with the live path matters: `NodeServer::collect_garbage` runs inside the node,
where the lock is already held and coordination is internal. So there are two GC paths
with the same report shape and different preconditions — do not conclude from the shared
`NodeGcReport` that the offline and online paths are interchangeable.

The applied path is also **not exhaustive**: each of the four stores is asked to collect
`now, limit, false`, so one invocation removes at most `limit` items per store (up to
1024) and repeated invocations are how a large backlog is drained.

**Reference and execution protection** is inherited from the stores themselves: blob
eligibility is `reference_count = 0` (a referenced blob is never a candidate), workspaces
must be past `expires_unix_ms`, and running executions keep their workspaces and blobs
referenced. This file does not add its own active-execution filter to GC — that
protection lives in the store implementations ([data-plane.md](data-plane.md),
[storage-journal.md](storage-journal.md)). Note that `gc` is a *separate* verb from
`drain`: `eggworkd gc` does not set the drain marker, so the operator must reason about
whether in-flight executions make a given candidate reachable themselves.

## Service and deployment verbs

`service_command` requires an explicit absolute `--service-config` on every verb and fails
closed when it is absent, with the comment "the service identity must name the exact
registered config path, so it is never guessed from the operator config." `--executable`
defaults to the canonicalized `current_exe` and is rejected if not absolute. The spec
comes from `deployment::service_spec_for_install(&executable, Some(&config_path))`, and
the same `args` slice is threaded into the manager constructor, so platform policy flags
are read there rather than by a separate parser.

Backend selection is **explicit and compile-time**, in `product_service_manager`:

- macOS: `--plist-path` (absolute) and `--launchd-domain user|system` are required;
  `--launchd-domain system` defaults the target to `system`, while `user` requires
  `--launchd-target gui/<uid>`; `--bootstrap-on-install` is a presence flag; timeout 60 s.
- Linux: `--unit-path` is required and must name a `*.service` file (the unit name is
  derived from its `file_name()`); `--scope` defaults to `system` and goes through
  `deployment::parse_systemd_scope`; `--enable` and `--no-reload` (reload defaults to on)
  are presence flags; timeout 60 s.
- Windows: refused, before any mutation.
- Anything else: "service management is unsupported on this platform".

`service_backend()` names the backend purely for the report: `systemd`, `launchd`,
`windows-scm-unsupported`, or `unsupported`.

**Adapter availability is never a platform support claim.** The typed adapter and the
typed policy are retained on every platform — the Windows path is compiled, it simply
refuses. What may be claimed supported is only where the native manager was actually
mutated on a real host. The in-file comment states the rule: Linux systemd is the
qualified path; "other platforms receive a structured diagnostic instead of a parallel
manager implementation."

**The Windows fail-closed refusal, and its ordering.** The source records the reason
precisely: hosted M004 qualification created the `eggwork-node` SCM registration and then
failed to start it with Windows error 1053, "the service did not respond to the start
request in a timely fashion." The cause is structural — `eggworkd` has no service-control
dispatcher, so the process the SCM launches never connects back to the manager. Because
the adapter *can* register the service, leaving the mutating path enabled would be
actively misleading: it would perform a real, partly successful, ultimately useless
mutation. Registering the daemon as a Windows service host is a product capability, not
a qualification fix, so it is a separate milestone. Until then every *mutating* `service`
verb on Windows returns a fixed refusal literal **before** any manager exists.
`deployment status` still reports host facts, and binary installation plus daemon runtime
remain qualified. `product_service_manager` is also called by `deployment apply`, so an
apply on Windows fails at the same point. Transaction mechanics are in
[deployment-lifecycle.md](deployment-lifecycle.md).

## `deployment apply` CLI surface

Accepted flags, all validated by `required_option` / `absolute_option` (every path option
must be absolute): `--installation-root`, `--release`, `--daemon`, `--service-config`, and
optionally `--helper`, `--previous-daemon-sha256`, `--previous-helper-sha256`, `--force`.

**It accepts already-local candidates only.** `--daemon` and `--helper` are absolute paths
to files that already exist on this host. The CLI builds a `Vec<CandidateSource>` of
`member` / `source` / `destination` triples (`MEMBER_DAEMON` → `DEST_DAEMON`, plus
`MEMBER_HELPER` → `DEST_HELPER`) and hands them to
`orchestrate_update_with_lifecycle_budgeted`. There is no URL argument, no release
resolution, no download, and no publish anywhere in the file.

**Platform conditions on the member set.** On Linux, `--helper` is *required* and must be
absolute — "Linux release apply requires --helper <absolute-candidate>" — because the
post-install check re-verifies helper compatibility. On every other platform, supplying
`--helper` is an error: "--helper is only valid for Linux release bundles."

**Fail-closed digest proof for replacement.** If *either* `--previous-daemon-sha256` or
`--previous-helper-sha256` is present, then `--previous-daemon-sha256` must be present and,
on Linux, `--previous-helper-sha256` must be present too; otherwise the CLI refuses with
"replacement requires previous SHA-256 values for every release member". In other words
you cannot prove *some* of a release and be trusted with the rest. The digests are parsed
by `parse_sha256` (exactly 64 hex characters, decoded to `[u8; 32]`) and installed into an
`eggup_core::ExactDigestVerifier`. Where the rule is actually *enforced* — i.e. what
"missing digest proof for an existing file" does — is inside Eggup, not in this file; the
CLI's contribution is that the verifier's expectation list is all-or-nothing per platform.

**The update request.** `drain_marker: Some(&drain_marker)`; `policy: UpdatePolicy { force,
..default }`; the quiescence probe is the closure `|| active_execution_count(config)`; and
the post-commit check runs `run_bounded(executable version)` with a timeout of
`min(remaining, 5 s)` and `MAX_VERSION_PROBE_BYTES`, then `check_installed_daemon_version`
against `CARGO_PKG_VERSION`, mapping the three `InstalledVersionCheck` variants to three
distinct failure strings. On Linux it additionally calls
`check_helper_compatibility_with_timeout(Some(&installed_helper), CARGO_PKG_VERSION, budget)`
against the same overall deadline. If the budget is already zero the check returns "post-install
check budget exhausted" rather than starting a probe.

**The report must carry:** `schema_version`, `release_id`, `artifact_disposition`,
`manual_artifact_recovery_required`, `lifecycle_restoration`, `final_ownership`,
`final_state`, `drain_remains_active` (hard-coded `true`), and three failure objects —
`artifact_failure`, `post_commit_failure`, `rollback_failure` — each `{phase, category,
detail}` and each `null` when absent. A reviewer should read the presence of all three
failure slots as the design intent: a rolled-back disposition, a committed-but-failed
check, and a failed rollback are three different operational stories and the operator must
be able to tell them apart from the JSON alone, since the exit code is 2 in every case.
[deployment-lifecycle.md](deployment-lifecycle.md) covers the transaction itself and
[release-qualification.md](release-qualification.md) the harness that drives it.

## Invariants and enforcement

| Invariant | Enforcement site | Test |
| --- | --- | --- |
| A group/other-writable or symlinked config is refused | `OperatorConfig::load` metadata gate | `invalid_configuration_is_rejected_without_exposing_secret_material` |
| A private key readable by group/other is refused | `trusted_private_key` | `valid_operator_config_builds_required_mtls_server_identity` |
| `config print` never emits the key path or client fingerprints | `OperatorConfig::redacted_json` | `redacted_config_print_hides_private_key_path_and_client_fingerprints` |
| An unknown operation name is a config error, not a missing permission | `validate` → `operation()` returning `None` | `valid_operator_config_builds_required_mtls_server_identity` |
| An unknown principal or ungranted operation is denied | `GrantAuthorizer::authorize` via `is_some_and` | No unit test in this file; see [server-node.md](server-node.md) |
| `doctor` agrees with the enforcement path on helper trust | `trusted_helper` → `verify_trusted_helper`, shared with the runner | `valid_operator_config_builds_required_mtls_server_identity` (adjacent); see [sandbox-helper.md](sandbox-helper.md) |
| Execution listings are bounded and paginate | `MAX_EXECUTION_PAGE`, `load_snapshots` `LIMIT 2049` + `Bounds` | `execution_listing_is_bounded_and_paginates_over_durable_records` |
| Symlinks are never followed when measuring a tree | `tree_bytes` `file_type().is_symlink()` check | `tree_size_never_follows_symlinks` |
| The reported metric set is fixed and bounded | `METRIC_NAMES` overlay filter | `metrics_report_uses_a_fixed_bounded_counter_set` |
| GC preview is read-only and reports its bound | `dry_run_gc` aggregate queries; no stores opened | `garbage_collection_preview_reads_only_and_reports_bounds` |
| `gc --apply` cannot race a running node | `acquire_maintenance_lock` `try_lock_exclusive` | `applying_gc_refuses_to_race_a_running_node` |
| The drain marker survives a process restart and `undrain` removes it | `set_persistent_drain` temp+rename+fsync; `NotFound` tolerated on removal | `drain_state_survives_process_restart_and_undrain_removes_marker` |
| Quiescence fails closed on unreadable state | `active_execution_count` returning `usize::MAX` | No test in this file; exercised by [release-qualification.md](release-qualification.md) |
| Windows service verbs mutate nothing | `#[cfg(windows)]` early return in `product_service_manager` | No test in this file; the comment records the M004 1053 evidence |
| Persistent drain stays set after a release, including success | `"drain_remains_active": true` in the apply report | `scripts/qualify_release.py` (`drain-visible-in-status`, `drain-cleared-explicitly`) |

## Test coverage

`operations.rs` has exactly 9 tests in its in-file `mod tests`, all library-level:
`redacted_config_print_hides_private_key_path_and_client_fingerprints`,
`drain_state_survives_process_restart_and_undrain_removes_marker`,
`tree_size_never_follows_symlinks`,
`execution_listing_is_bounded_and_paginates_over_durable_records`,
`garbage_collection_preview_reads_only_and_reports_bounds`,
`applying_gc_refuses_to_race_a_running_node`,
`metrics_report_uses_a_fixed_bounded_counter_set`,
`invalid_configuration_is_rejected_without_exposing_secret_material`, and
`valid_operator_config_builds_required_mtls_server_identity`. They cover config
load/validate, redaction, drain, sizing, pagination, both GC modes, and metrics. They
build their fixture config inline, which is why the field set is duplicated in the test
helper and why a new `OperatorConfig` field is a compile-time reminder there.

**`bin/eggworkd.rs` has no `#[cfg(test)]` module at all.** Every CLI concern is therefore
untested at the unit level: argv parsing and the `--config` splice, `parse_option`
coercion, `service_command`'s fail-closed `--service-config`, the platform policy flag
sets, `parse_sha256`, the `deployment apply` all-or-nothing digest rule, and the exit-code
discipline. That is the single largest coverage gap in this component, and it is why the
release harness matters so much.

`scripts/qualify_release.py` drives the *installed* binary through operator commands
(per its own module docstring). At grep level it invokes `version` (both for the
installed daemon and for a prior candidate), `status`, `drain`, and `undrain`, builds
`eggworkd service` argv against an installed executable (install, then `status`, then
cleanup/uninstall), and runs `deployment apply` against two generations. It asserts on
the exact JSON shapes this CLI produces: the service mutation keys
`{schema_version, service_id, platform, backend, operation, completed}`, the
`drain_remains_active is True` field, and `drain-visible-in-status` /
`drain-cleared-explicitly` / `drain-cleared` transitions, plus a refusal case asserting
both a nonzero return code and that `draining` is present in the output. I read this
harness only by targeted grep, so the list above is grep-verified but not
exhaustively so.

**Honest gaps.** No unit test for `GrantAuthorizer` itself; no test for
`active_execution_count`'s fail-closed branches; no test for `storage_summary`'s WAL/shm
accounting; no test for the three `inspect_*` functions or the blob size cross-check; no
test for `start_server`; and no test at all in the binary. The macOS launchd flag matrix
and the Windows refusal are, by their nature, only exercisable on those hosts.

## Review focus

- **No `--help`/`-h`, and JSON formatting is not uniform.** `eggworkd --help` is parsed as
  a command, fails the `--config` lookup, and returns usage on stderr with exit 2 — easy to
  mistake for a crash. Separately, `config print` and `run` bypass the `output()` helper, so
  pretty-printing differs by verb; consumers should parse, not diff lines.
- **Exit code 2 is the only failure signal, and it arrives *with* valid JSON on stdout.**
  `doctor` and `config validate` print a complete report and then fail, so "exit 2" does not
  mean "no output". And because there is one failure code, a rolled-back apply, a
  committed-but-failed-check apply, and a failed rollback are all exit 2 — only the JSON
  body distinguishes them, and nothing forces a script to read it.
- **TOCTOU between metadata and open.** `load`, `read_certificates`,
  `trusted_private_key`, and `require_regular_file` each call `symlink_metadata` and then
  `File::open`/`Connection::open` as two separate operations. A swap in between passes the
  symlink check. O_NOFOLLOW-style opening would close it.
- **`tree_bytes` is unbounded.** `storage summary` walks both roots with no limit and no
  timeout. On a large or pathological tree this is a long, uninterruptible-ish operation
  invoked by a single keystroke.
- **Execution paging is in-memory.** `execution_page` decodes up to 2048 full snapshots
  and fails closed with `Bounds` above that. The `LIMIT 2049`/`> 2048` boundary means a
  node at exactly 2048 records works and 2049 fails — a cliff, not a gradient. `status`
  and `executions list` both depend on it.
- **`--limit` bounds are samples, not totals.** `gc` preview at limit N reporting zero
  candidates does not mean the store is clean. The report does not say so.
- **Unbounded trailing arguments are ignored.** No verb checks arity, so a typo'd or
  injected argument is silently dropped. First-occurrence wins for duplicate flags.
- **Error flattening loses diagnosability.** Every `OperationsError` becomes a fixed
  string, so "configuration is invalid" cannot be distinguished from a bad path. That is
  good for secrecy and bad for operators; `doctor` is the intended fallback.
- **`--force` bypasses quiescence** while `drain_remains_active` is still reported `true`.
  The report therefore does not by itself distinguish a quiesced release from a forced
  one; a reader could conclude more than the JSON supports.
- **The Windows refusal ordering must stay ahead of manager construction,** and the
  `#[cfg]`-gated `option_value` (Linux, macOS, Windows only) deserves the same scrutiny: the
  safety argument depends on the `#[cfg(windows)]` block returning before any mutation is
  possible at *both* call sites — `service` and `deployment apply` — and the
  "unsupported platform" branch is unreachable from `deployment apply` on any other
  target. Re-check both on any refactor.
- **Redaction covers two fields.** `tls.certificate_chain`, `tls.client_ca`, and
  `clients[].principal_id` are printed in the clear. That is a considered trade-off, but
  the file does not state why *those* three are safe to publish and the two redacted ones
  are not.
- **`is_persistently_draining` is a bare `is_file` check.** It never validates the
  `draining\n` contents, and a directory at that path reads as "not draining". Relatedly,
  `drain_path` uses `with_extension`, which *replaces*: a `database_path` ending in
  `.drain` would make the marker the database itself, and `set_persistent_drain` would
  `rename` the text `draining\n` over the execution database; a `.lock` suffix would
  collide with the GC lock. `validate()` rejects neither. This deserves a guard.

## Related

- [overview.md](overview.md)
- [server-node.md](server-node.md)
- [storage-journal.md](storage-journal.md)
- [data-plane.md](data-plane.md)
- [deployment-lifecycle.md](deployment-lifecycle.md)
- [release-qualification.md](release-qualification.md)
- [ci-guardrails.md](ci-guardrails.md)
- [runner-execution.md](runner-execution.md)
- [sandbox-helper.md](sandbox-helper.md)
- [../README.md](../README.md)
