# storage journal (store.rs)

`crates/eggwork-server/src/store.rs` (895 lines) is the node's durable memory of
executions: it is the single place where an accepted execution identity, its
lifecycle state, its ownership-lease evidence, and its event history survive a
process crash. It is a private module (`mod store;` at `lib.rs:9`), backed by a
single `rusqlite` connection (rusqlite 0.40.2 / libsqlite3-sys 0.38.2, bundled)
at a caller-supplied path, and it is reached only through `eggwork-server`'s
public surface (`NodeServer`, `eggworkd`, `operations`). Every execution
identity is written here *before* a runner process is spawned, so the storage
layer — not the HTTP layer — is what makes `POST /execute` idempotent, fences
stale controllers, and lets a restarted node refuse to replay uncertain work.

## Responsibility boundary

Owns: the durable `(execution_id, generation)` identity set and its reservation
verdict (`Created` / `Existing` / `Conflict` / `StaleGeneration` /
`StorageFull`); the latest `ExecutionSnapshot` JSON blob per identity
generation plus the denormalized `state` column used for terminal/active
predicates; the request digest, canonicalization version, and principal
attribution that make a duplicate submit decidable; the hashed lease token,
wall-clock lease expiry, and last renewal id; the generation-scoped, gap-free,
bounded event journal; node metric counters; and the startup reconciliation
that converts persisted nonterminal state to `Interrupted`.

Does not own:

- blobs, workspaces, artifacts, or manifest retention. Those live in
  `blob.rs`, `workspace.rs`, `artifact.rs` with their own SQLite files
  (`<db>.artifacts.sqlite`, `<workspace_root>/metadata.sqlite`,
  `<blob_root>/metadata.sqlite`). `store.rs` stores no workspace or artifact
  references, only the `ExecutionSnapshot` the server hands it.
- stdout/stderr as durable data. `ExecutionEventKind::Stdout`/`Stderr` payloads
  are journaled in bounded chunks and then trimmed away; the authoritative
  capture path is artifact/blob storage.
- any scheduling, placement, retry, or admission decision. `reserve` decides
  only "is this identity free and is this generation ordered"; the server
  separately enforces `MAX_RECENT_EXECUTIONS`, the `max_active_executions`
  semaphore, draining, and capability gates before calling it.
- the process lifecycle. Nothing here spawns, signals, or adopts a child.
- the operator-visible drain marker, which is a filesystem sentinel read by
  `operations.rs`, not a row in this database (see
  [Drain state and the exclusive lock](#drain-state-and-the-exclusive-lock)).

It is node-local: no cross-node event log, no replication. The file is opened
with mode `0600` on Unix when it does not yet exist (`store.rs:64-77`).

## Schema

There is **no schema-version table and no migration mechanism.** The DDL is a
single `execute_batch` of `CREATE ... IF NOT EXISTS` statements issued on every
`ExecutionStore::open` (`store.rs:79-114`), preceded by three PRAGMAs:

```sql
PRAGMA journal_mode=WAL;
PRAGMA synchronous=FULL;
PRAGMA foreign_keys=ON;
```

There is no `PRAGMA user_version` read or write, and no `schema_version` column.
`ExecutionSnapshot.schema_version` (`eggwork-core/src/lib.rs:1117-1123`) is
*inside* the JSON blob, is set to `API_SCHEMA_VERSION` (`1`, `lib.rs:48`) at
construction, and is not validated on read — `serde_json::from_slice` will
deserialize any `u16` into it.

On a version mismatch there is no explicit handling: a database whose
`executions` table lacks a column the current DDL expects fails at first
`prepare`/step time with a raw `rusqlite` error surfacing as
`StoreError::Sql`, and `CREATE TABLE IF NOT EXISTS` will not add missing
columns to an existing table. Canonicalization *is* versioned, separately, via
`eggwork_core::CANONICAL_REQUEST_VERSION = 2` and
`CANONICAL_WORKSPACE_REQUEST_VERSION = 3`, and that version is stored in
`executions.canonical_version` and compared in `reserve` and in the `execute`
handler (`lib.rs:1670-1676` → 409 `canonicalization_version_mismatch`).

### `executions`

| Column | Type | Null | Default | Constraint / use |
|---|---|---|---|---|
| `execution_id` | `TEXT` | NOT NULL | — | First half of the primary key; opaque `ExecutionId` string |
| `generation` | `INTEGER` | NOT NULL | — | Second half of the primary key; `ExecutionGeneration::get() as i64` |
| `canonical_version` | `INTEGER` | NOT NULL | — | `u16` canonicalization version (2 or 3) |
| `digest` | `TEXT` | NOT NULL | — | Lowercase hex SHA-256 of the canonical request; idempotency key |
| `principal_id` | `TEXT` | NOT NULL | — | Owning `PrincipalId`; the fencing predicate for reads |
| `lease_token_hash` | `TEXT` | NOT NULL | — | `lease_hash(LeaseId)`, 64 hex chars; the raw token is never stored |
| `lease_expires_unix_ms` | `INTEGER` | NOT NULL | — | Wall-clock deadline; monotonic, never decreasing |
| `last_renewal_id` | `TEXT` | nullable | `NULL` | Idempotency key for a single renewal |
| `state` | `TEXT` | NOT NULL | — | One of `Accepted`, `Preparing`, `Running`, `Cancelling`, `Succeeded`, `Failed`, `Cancelled`, `TimedOut`, `Interrupted` |
| `next_sequence` | `INTEGER` | NOT NULL | `1` | Next event sequence to assign; incremented to `sequence + 1` in the same transaction as each insert |
| `snapshot_json` | `BLOB` | NOT NULL | — | `serde_json::to_vec(&ExecutionSnapshot)` |
| `created_unix_ms` | `INTEGER` | NOT NULL | — | Set once at reservation |
| `updated_unix_ms` | `INTEGER` | NOT NULL | — | Bumped by `commit_event` and `update_lease` |

PRIMARY KEY `(execution_id, generation)`.

Indexes:

| Index | Definition | Serves |
|---|---|---|
| `executions_latest` | `ON executions(execution_id, generation DESC)` | `ORDER BY generation DESC LIMIT 1` in `reserve`, `load_snapshot(None)`, `load_snapshot_for_principal(None)` |
| `sqlite_autoindex_executions_1` (SQLite-implied) | unique on the composite PK | the `WHERE execution_id = ?1 AND generation = ?2` point lookups |

Note that `snapshot_json` duplicates `execution_id`, `generation`, `state`, and
`result`; the denormalized `state`/`generation` columns exist so the store can
make terminal and latest-generation decisions without decoding JSON.

### `execution_events`

| Column | Type | Null | Constraint / use |
|---|---|---|---|
| `execution_id` | `TEXT` | NOT NULL | FK column |
| `generation` | `INTEGER` | NOT NULL | FK column |
| `sequence` | `INTEGER` | NOT NULL | Third part of the primary key; generation-scoped, starts at 1 |
| `encoded` | `BLOB` | NOT NULL | `serde_json::to_vec(&ExecutionEvent)`; the embedded `sequence` field always equals the row key |

PRIMARY KEY `(execution_id, generation, sequence)`.
FOREIGN KEY `(execution_id, generation) REFERENCES executions(execution_id,
generation) ON DELETE CASCADE`.

The FK is enforced because `PRAGMA foreign_keys=ON` is set per connection, and
the store deletes event rows explicitly in `trim_events` rather than relying on
cascade (no code path deletes an `executions` row). The composite-PK
autoindex also serves `MIN(sequence)`, `ORDER BY sequence ASC`, and the
`sequence > ?3` range scan in `load_page`.

### `node_metrics`

| Column | Type | Null | Constraint / use |
|---|---|---|---|
| `name` | `TEXT` | PRIMARY KEY | Counter name; `increment_metric` rejects empty or `> 64` bytes |
| `value` | `INTEGER` | NOT NULL | `CHECK(value >= 0)`; saturating upsert |

Upsert SQL is `ON CONFLICT(name) DO UPDATE SET value=CASE WHEN value >
9223372036854775807 - excluded.value THEN 9223372036854775807 ELSE value +
excluded.value END`, so a counter saturates at `i64::MAX` rather than wrapping
into a negative value that the `CHECK` would reject.

## Identity columns

Four independent orderings are stored on the same row, and conflating any of
them breaks a distinct guarantee:

| Column | Type in SQLite | Rust type | Ordering it encodes |
|---|---|---|---|
| `execution_id` | `TEXT` | `ExecutionId` (bounded string) | *Which* logical execution. Stable across generations. |
| `generation` | `INTEGER` | `ExecutionGeneration(u64)`, non-zero | *Who owns it now* — a monotonically increasing ownership epoch (`eggwork-core/src/lib.rs:111-124`). |
| `next_sequence` / `execution_events.sequence` | `INTEGER` | `EventSequence(u64)` (`lib.rs:129-137`) | *Observation order* of events within one generation. |
| `digest` | `TEXT` | `String` (64 hex chars) | *What was asked for* — content identity of the canonical request. |
| `lease_token_hash` | `TEXT` | `String` (64 hex chars) | *Who is allowed to control it* — bearer authority. |

**Ownership order is separate from identity** because ADR-0003 decisions 5/6 make
generations "monotonically ordered ownership epochs" and fence stale
controllers: a new generation for the same `execution_id` is a *new owner of the
same identity*, not a new identity. The store enforces one ordering rule
(`store.rs:166-179`) — with no rows only `generation == 1` is legal; otherwise
the generation must be `latest + 1` **and** the latest `state` must already be
one of `Succeeded | Failed | Cancelled | TimedOut | Interrupted`, else
`ReserveResult::StaleGeneration`. Generations never skip, regress, or overlap.

**Sequence is separate from generation** because event streams must not be
confusable across epochs (ADR-0003 decision 15). Both the `execution_events`
`PRIMARY KEY` and every query in `load_page`/`load_all` scope by
`(execution_id, generation)`, so a generation-1 cursor cannot be silently
satisfied by generation-2 rows; the `sequence` embedded in the JSON always
equals the row key, so a client can verify it.

**The digest is the idempotency key.** `request_digest_with_workspace`
(`lib.rs:1605-1616`) normalizes unordered fields (sort environment by name,
metadata by key then value, declared outputs by path then required), prefixes a
domain-separated literal - `b"eggwork-execution-spec\0canonical-json-v2\0"`, or
`...v3\0` when a `workspace_id` participates - serializes the normalized typed
struct, and SHA-256s it. Caller JSON field order therefore cannot change the
digest, and the stored version makes a future algorithm change detectable rather
than silently mismatched.

Load-bearing consequences: identity is *not* just `execution_id`, so reusing an
id at generation 2 is a takeover rather than a collision; the digest is stored
but the normalized request is **not** (deliberate - persisting normalized
environment values would write plaintext secrets for diagnostics, per the
closure); and both `principal_id` and `lease_token_hash` sit in the `reserve`
conflict tuple without being part of the primary key, so a cross-principal
collision is a `Conflict` and a second controller cannot hijack an identity it
merely knows the digest of.

## Reservation and idempotency

`ReserveResult` (`store.rs:38-45`):

| Variant | Meaning | Server mapping (`lib.rs:1789-1851`) |
|---|---|---|
| `Created` | Row inserted; this caller owns the identity. | admit, `executions_accepted`, spawn `run_execution` |
| `Existing(Box<ExecutionSnapshot>)` | Identical `(canonical_version, digest, principal_id, lease_token_hash)` already reserved for this `(id, generation)`. | re-attach via `recovered_record`, return the event stream from cursor 0; no spawn |
| `Conflict` | Row exists but any of the four fields above differs. | 409 `execution_identity_conflict` |
| `StaleGeneration` | No row, or generation not `latest + 1`, or latest generation not terminal. | 409 `generation_mismatch` |
| `StorageFull` | `COUNT(*) FROM executions >= MAX_EXECUTION_IDENTITIES` (2048). | 503 `storage_exhausted` |

`reserve` opens with
`connection.transaction_with_behavior(TransactionBehavior::Immediate)`
(`store.rs:132-133`) - a write transaction, so the write lock is taken at
`BEGIN`, not at first write. Inside that one transaction it (1) point-reads the
existing row for `(id, generation)` and returns `Existing`/`Conflict`, where
`Existing` decodes `snapshot_json` so a decode failure is `StoreError::Json`
rather than a silent treat-as-new; (2) otherwise reads the latest generation and
state and applies the ordering rule; (3) counts rows and applies the 2048 cap;
(4) inserts with `next_sequence = 1` and `created_unix_ms = updated_unix_ms =
now`.

### Can two callers with the same digest both be admitted?

No, and the guarantee is structural rather than advisory:

- The store has exactly **one** SQLite connection, `Arc<Mutex<Connection>>`
  (`store.rs:34-36`), and every method clones the `Arc` into a `spawn_blocking`
  closure and takes `connection.lock()`. Two concurrent `reserve` calls cannot
  interleave inside the transaction body at all; the second blocks on the
  `std::sync::Mutex` until the first commits.
- Even with a shared connection, `TransactionBehavior::Immediate` plus the
  composite `PRIMARY KEY` means a racing `INSERT` hits
  `SQLITE_CONSTRAINT_PRIMARYKEY`, and the read-before-write ordering means it
  would already have returned `Existing`.
- The loser therefore receives `Existing` and the server replays that execution's
  event stream; `run_execution` is spawned only on the `Created` arm
  (`lib.rs:1852-1882`).

The server adds a weaker second layer - `execute` holds the in-memory
`state.executions` mutex and a `max_active_executions` permit across the reserve,
dropping the permit on `Existing` - which is not needed for the digest guard.
Note the `digest` alone is *not* a key: the conflict tuple also carries
`canonical_version`, `principal_id`, and `lease_token_hash`, so a caller cannot
replay a digest without presenting the same lease token it first used.

## Lease records

Lease state lives in three columns of the same `executions` row:

| Column | Meaning |
|---|---|
| `lease_token_hash` | `lease_hash(LeaseId)`; the fencing credential |
| `lease_expires_unix_ms` | Wall-clock deadline, `unix_millis() + lease_ttl` at reservation and at each renewal |
| `last_renewal_id` | Idempotency key for the most recent renewal; nullable, defaults to unset |

There is no separate lease table and no lease history. A new generation writes a
new row and therefore a new lease record; the previous generation's row keeps
its own hash and expiry, so a stale holder's token still "matches" its own
generation but fails the `latest.generation == handle.generation` check `control`
applies *before* mutating anything (`lib.rs:2058-2078`).

`lease_hash` (`store.rs:632-634`) is
`hex::encode(sha2::Sha256::digest(lease.as_bytes()))` — unkeyed SHA-256, 64
lowercase hex chars; the raw `LeaseId` is never persisted. This is credential
hygiene: `eggwork-core` documents `LeaseId` as "bearer authority" and redacts it
in `Debug` (`ExecutionHandle::fmt`), and a database file is the most likely thing
to be copied off-box. The store only ever performs hash *equality*, so no
timing-safe comparison is required for the value to be non-recoverable.

**What an attacker or stale holder learns.** The guarantee is asymmetric: a
reader of the file with a candidate `LeaseId` can confirm it in one SHA-256, so
the database is an *offline verification oracle* for candidate tokens, not a way
to recover one (recovery is a preimage problem). Mitigations are
filesystem-level - the file is created `0600` on Unix, and the exclusive `.lock`
prevents a second node process writing concurrently. A stale holder learns, by
reading the row, only the persisted wall-clock expiry: it cannot extend that
without a renewal passing the `current_expiry <= unix_millis()` guard, and it
cannot discover the *next* generation's token.

**TTL and expiry representation.** Expiry is `unix_millis() + lease_ttl`, i.e.
`Duration::as_millis().min(i64::MAX as u128) as i64` added with `saturating_add`
(`lib.rs:1661-1662`, `lib.rs:2125-2126`). `unix_millis` (`store.rs:636-642`) is
`SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
.min(i64::MAX as u128) as i64` — pure wall clock, saturating, deliberately free
of a monotonic component. The *live* deadline is monotonic and in-memory only
(`LeaseState.expires_at: tokio::time::Instant`, `lib.rs:1902-1906`); the
persisted value is evidence and renewal-idempotency state, per the closure's
note that on restart every uncertain execution is interrupted "regardless of
the remaining wall-clock lease".

**Renewal** — `update_lease` (`store.rs:351-406`) in one `Immediate`
transaction:

1. reads `(lease_token_hash, lease_expires_unix_ms, last_renewal_id, state)`;
2. returns `None` (→ 409 `lease_expired` from the caller) if the row is absent,
   the hash differs, the state is terminal, or
   `current_expiry <= unix_millis()` — i.e. **an already-expired lease cannot
   be resurrected**;
3. if `last_renewal_id == Some(renewal_id)`, commits and returns the persisted
   `current_expiry` unchanged, so replaying a renewal id neither double-extends
   nor errors;
4. otherwise `expires = current_expiry.max(lease_expires_unix_ms)` — the
   deadline is monotonic, so a delayed renewal can only shorten the gap, never
   shorten the lease — and writes it with the new `last_renewal_id` under the
   predicate `... AND lease_token_hash = ?3`.

The caller's requested expiry is always `now + lease_ttl`, so step 4 means a
renewal *extends* by up to one TTL and a replayed/older request is clamped to
the existing deadline. The `AND lease_token_hash = ?3` predicate makes the
write itself fenced even though the read already compared it.

**Transitions that clear or replace a lease.** The store never nulls or erases
`lease_token_hash` or `lease_expires_unix_ms` — not on terminal transition
(`commit_event` only updates `state`, `next_sequence`, `snapshot_json`,
`updated_unix_ms`), not on renewal. The only replacement is the `INSERT` of a
new generation row. Effectively the lease is immutable per generation and
"clearance" is expressed by the state column being terminal, which makes
`update_lease` return `None`. The in-memory `lease.expired` flag and the
`CancellationToken` (`monitor_lease`, `lib.rs:1932-1954`) are the *runtime*
lease state; the store only supplies the durable deadline.

## State machine persistence

`ExecutionState` (`eggwork-core/src/lib.rs:878-887`) has nine variants.
`state_name` (`store.rs:644-656`) maps each to its SQLite spelling verbatim
(`Accepted`, `Preparing`, `Running`, `Cancelling`, `Succeeded`, `Failed`,
`Cancelled`, `TimedOut`, `Interrupted`) and `is_terminal_name` (`store.rs:658-663`)
recognizes the last five.

In-memory `ExecutionSnapshot` (`{ schema_version, execution_id, generation,
state, result: Option<ExecutionResult> }`) is serialized whole into
`snapshot_json`; the `state` column is written from the *caller's* proposed
snapshot via `state_name(&snapshot.state)`, never from the row that was read
back (`store.rs:326-337`). So the stored snapshot and the stored state column
are always consistent with each other by construction.

### Which transitions the store actually enforces

`store.rs` enforces only three things, all as *predicates*, not a state machine:

1. **No mutation after terminal** — `commit_event` returns `Ok(None)` if the
   stored `state` `is_terminal_name` (`store.rs:310-313`); `update_lease`
   returns `None` on a terminal row; `reserve` requires the previous
   generation to be terminal before advancing.
2. **Generation ordering** as described above.
3. **Row must exist** — every mutation is a no-op returning `None` for an
   unknown `(id, generation)`.

It does **not** check that `proposed.state` is a legal successor of the stored
state — a caller could move `Running → Preparing` and the store would accept it.
Legality is a caller convention: `publish` (`lib.rs:2569-2595`) holds the
in-memory write lock, refuses an already-terminal in-memory state, and is the
only production writer. The sequence it actually produces:

| Step | Site | `state` written | Event kind | Sequence |
|---|---|---|---|---|
| reserve | `execute` → `reserve` | `Accepted` | *(none — no event yet)* | `next_sequence = 1` |
| admit | `publish` | `Accepted` | `State(Accepted)` | 1 |
| start | `run_execution` → `publish` | `Running` | `State(Running)` | 2 |
| cancel request | `control` → `publish` | `Cancelling` | `State(Cancelling)` | 3 |
| output chunks | `run_execution` → `publish` | `Running` (re-asserted) | `Stdout`/`Stderr`/`Diagnostic` | interleaved, each consuming one sequence |
| outcome | `run_execution` → `publish_terminal` | terminal + `result` | `State(terminal)` | last |

`Preparing` is never written by the server; it exists in `state_name`, in
`recover()`'s `IN` list, and in the `operations.rs` active filters, so a
hand-seeded or future row can carry it.

### Terminal state is recorded exactly once

Two independent guards, in order: `publish` takes `record.snapshot.write()` and
returns early if the in-memory state `is_terminal` (`lib.rs:2575-2578`); then
`commit_event` re-reads the stored `state` inside the transaction and returns
`Ok(None)` if it is terminal (`store.rs:306-313`), rolling back, with `publish`
treating `Ok(None)` as "already terminal — do not touch memory, do not
broadcast".

The terminal `UPDATE` of `executions.state`/`snapshot_json` and the `INSERT` of
the terminal event are the **same `Immediate` transaction**
(`store.rs:294-344`), so a durable terminal outcome and its journal entry cannot
diverge. Memory is published only after `commit_event` returns
`Ok(Some(event))` — "in-memory state changes only after durable commit; a failed
commit drains the node and cancels that execution" (`lib.rs:2590-2593`).


### What makes a crash mid-execution recoverable

`recover()` (`store.rs:535-604`), called once from `NodeServer::start`
(`lib.rs:327-333`) immediately after `ExecutionStore::open` and before the
server binds, inside a single `Immediate` transaction:

- selects every `snapshot_json` where
  `state IN ('Accepted', 'Preparing', 'Running', 'Cancelling')`;
- rewrites each in memory to `state = Interrupted` with a synthesized
  `ExecutionResult { state: Interrupted, exit_code: None, failure: Some(ExecutionFailure::Interrupted), stdout_bytes: 0, stderr_bytes: 0, stdout_omitted: 0, stderr_omitted: 0, cleanup_warning: None, finalization_failure: None, artifact_count: 0, sandbox: None, resources: None }`;
- re-reads `next_sequence` per row, appends one
  `State(Interrupted)` event at that sequence, advances `next_sequence`, writes
  the new snapshot and `updated_unix_ms`, and calls `trim_events`;
- commits, returning the rewritten snapshots (also used to bump
  `terminal_interrupted` by the count).

Properties this buys: **no replay** (nothing is reconstructed or respawned; the
returned snapshots only count a metric — ADR-0003 decision 14, "restart does not
automatically rerun an interrupted execution"); **idempotence** under repeated
recovery (a second `recover()` matches no rows because the first pass made them
all terminal); **crash-consistent cut points** (each state+event pair commits
atomically, so a crash between the terminal `UPDATE` and the event `INSERT` is
impossible, and a crash before either leaves the row nonterminal for the next
boot); and an **honest outcome** (the synthetic result reports zeros rather than
fabricating a child's exit code — "emit recovery evidence without fabricating
process exit"). **Not attempted:** process adoption or proof that a child is
dead; a child that outlived its parent node is neither reaped nor adopted here,
and the store's answer is conservative interruption regardless of the remaining
lease.

## Event journal

### Append path

`commit_event(snapshot, kind)` (`store.rs:286-349`) in one `Immediate`
transaction: (1) `SELECT state, next_sequence` for `(id, generation)` - `None` or
terminal rolls back and returns `Ok(None)`; (2) build
`ExecutionEvent { sequence: EventSequence::new(sequence as u64), kind, metadata:
EventMetadata { fields: vec![] } }` - metadata is always empty, the store never
accepts caller-supplied metadata; (3) `event.validate()`, then
`serde_json::to_vec`, rejecting `encoded.len() > MAX_RETAINED_EVENT_BYTES` with
`StoreError::InvalidEvent` (the transaction is dropped, i.e. rolled back);
(4) `UPDATE executions SET state = state_name(&snapshot.state), next_sequence =
sequence + 1, snapshot_json = to_vec(&snapshot), updated_unix_ms = now`;
(5) `INSERT INTO execution_events (execution_id, generation, sequence, encoded)`;
(6) `trim_events`; (7) `commit`, returning `Some(event)`. The event is broadcast
to live subscribers only after the commit, in `publish` (`lib.rs:2585-2588`).


### Sequence assignment and gap-freedom

The sequence is *read from the row inside the same transaction that writes the
next value* (`next_sequence = sequence + 1`) and is protected by a composite
`PRIMARY KEY`. With one connection behind a `Mutex` and an `Immediate`
transaction, two concurrent appends cannot observe the same `next_sequence`:
either the mutex serializes them, or the second `BEGIN IMMEDIATE` blocks on the
write lock (up to `busy_timeout` = 2 s) and re-reads the advanced value. So
within a generation the journal is `1, 2, 3, …` with no gaps and no duplicates.
`trim_events` deletes only the *lowest* sequence, so trimming opens a hole at the
*front* (`base_sequence`) and never mid-journal. Gap-freedom is per-generation
only: generation 2 restarts at 1.

### Payload storage

`encoded` is a `BLOB` holding the JSON of the whole `ExecutionEvent`, including
the duplicated `sequence` field. Kinds are `State(ExecutionState)`,
`Stdout(Vec<u8>)`, `Stderr(Vec<u8>)`, `Diagnostic(String)`.
`ExecutionEvent::validate` (`eggwork-core/src/lib.rs:1043-1057`) caps the raw
payload at `MAX_EVENT_CHUNK_BYTES = 64 * 1024`; `EventMetadata::validate` caps
metadata at 64 fields / 128-byte keys / 2048-byte values, but the store always
writes `fields: vec![]`, so only the 64 KiB payload cap is reachable.
`ExecutionEventKind` and `EventMetadata` both redact contents in `Debug`.

### `EventPage` and the `after` cursor

`EventPage { base_sequence: u64, next_sequence: u64, events: Vec<ExecutionEvent> }`.

`load_page(id, generation, after_sequence)` (`store.rs:408-460`):

1. reads `next_sequence`; `None` → `Ok(None)` (caller answers 404);
2. `base_sequence = COALESCE(MIN(sequence), next_sequence).max(1)`;
3. if `after_sequence.saturating_add(1) < base_sequence`, return an **empty**
   `events` vector with the real `base_sequence` — a deliberate "no partial
   replay" signal;
4. otherwise `SELECT encoded ... WHERE sequence > ?3 ORDER BY sequence ASC`.
   There is **no `LIMIT`** on this query; the page size is bounded only
   indirectly by `MAX_RETAINED_EVENTS`.

`after` is exclusive and generation-scoped. `events_route` requires both
`generation` and `after` as query parameters, rejects `after > i64::MAX as u64`
with 416, and requires a `lease` query parameter that must match
`lease_token_hash` and `principal_id` (`lib.rs:2253-2275`). `event_response`
(`lib.rs:2303-2349`) then maps page shape to HTTP:

| Condition | Response |
|---|---|
| `after.saturating_add(1) < page.base_sequence` | 410 `history_expired`, and `increment_metric("event_history_resync")` |
| `after >= page.next_sequence` | 416 `cursor_ahead` |
| otherwise | `event_stream_response` — durable replay, then live `broadcast::Receiver`, ending after a terminal event |

`load_page`'s empty page and `event_response`'s 410 are the same predicate
computed twice; `event_response` is the one that speaks HTTP.

### Retention / pruning

`trim_events` (`store.rs:607-630`) runs inside the append transaction after the
insert. It loops `SELECT COUNT(*), COALESCE(SUM(length(encoded)), 0)` for the
generation, stopping when both are in bounds and otherwise deleting the single
row at `MIN(sequence)`. The journal is therefore a **ring buffer per
generation** retaining at most the newest 256 events / 512 KiB. Trimming one
row at a time is O(evicted) statements inside the caller's transaction, and
`MIN(sequence)` on the composite-PK autoindex keeps each delete cheap.

### What a resuming client can and cannot miss

Can never miss: any retained event with sequence `> after` — the retained window
is contiguous from `base_sequence` to `next_sequence - 1` by construction, so
reconnect within retention is continuous. Cannot get: already-trimmed events —
the client gets 410 `history_expired` and must resynchronize from the snapshot,
not from a partial window; that 410 is the only way to distinguish "trimmed"
from "never existed", which is why the store refuses to return a partial page.
The *live* broadcast channel is a separate, much smaller bound
(`EVENT_CAPACITY = 32`, `lib.rs:50`): a lagging consumer gets
`broadcast::error::RecvError::Lagged` and its stream is terminated with an error
rather than a gap (`lib.rs:2628-2635`). The two layers can therefore disagree
about what a client has seen — durable history 256 events, live fan-out 32.

## Transactions and concurrency

**Connection model: one connection, no pool.** `ExecutionStore` is
`#[derive(Clone)]` over `Arc<Mutex<Connection>>` with a `std::sync::Mutex`
(`store.rs:33-36`). Every method clones the `Arc` into a
`tokio::task::spawn_blocking` closure, so no async task holds a `rusqlite` handle
across an await and no `!Send` type crosses a thread boundary.
`spawn_blocking` keeps blocking SQLite calls off the async worker threads; it
buys no parallelism. The consequence is that **all store access is fully
serialized in-process**: a `SELECT COUNT(*)` during `reserve` blocks
`commit_event`, and a slow WAL commit stalls unrelated readers.

**Busy handling.** `busy_timeout(Duration::from_secs(2))` is set at open
(`store.rs:78`). There is no application-level retry anywhere in `store.rs`:
`SQLITE_BUSY` after 2 s becomes `StoreError::Sql` and, at the server, a 500
`ServiceError::internal("execution store unavailable")`. Since the mutex waits
unboundedly and is normally the only contention source, the 2 s timeout mostly
covers the cross-process case (the read-only CLI connections).

**Which operations are transactional:**

| Method | Transaction | Notes |
|---|---|---|
| `reserve` | `Immediate` | read-check + `INSERT` |
| `commit_event` | `Immediate` | terminal check + snapshot `UPDATE` + event `INSERT` + `trim_events` |
| `update_lease` | `Immediate` | lease read + expiry/hash/idempotency check + fenced `UPDATE` |
| `recover` | `Immediate` | one transaction for the whole interrupted set |
| `lookup` | none (autocommit single `SELECT`) | |
| `load_snapshot_for_principal` | none | two shapes, both single `SELECT`s |
| `load_page` | none | **three** separate statements: `next_sequence`, `MIN(sequence)`, then the event range — not a consistent snapshot |
| `load_all` | none | single `SELECT` |
| `increment_metric` | none (explicit single-transaction batch) | validates every name *before* opening the transaction, so one bad name rejects the whole batch without partial application |

**Windows where an inconsistency can be observed:**

- `load_page` reads `next_sequence` and `MIN(sequence)` in separate autocommit
  statements, so a concurrent `commit_event` between them can pair a
  `base_sequence` with a stale `next_sequence`. Worst case is a spurious 410 or a
  `next_sequence` that understates reality by one — the client re-sees the newer
  event on the next cursor round, so a benign duplicate, never a gap.
- `load_all` at startup (`lib.rs:346-352`) reads every snapshot to seed the
  in-memory map. It runs *after* `recover()` with no other writer, so it is safe
  there; called at any other time it would be a full-table read under the mutex.
- The `lookup`-then-`reserve` pair in `execute` (`lib.rs:1664-1763`) is
  deliberately redundant; it exists only to produce the finer-grained 403
  `forbidden` and 409 `canonicalization_version_mismatch` responses that
  `ReserveResult::Conflict` cannot distinguish. The window between the two calls
  is closed by `acquire_state_lock` (one node process), not by the store.
- `update_lease`'s read-then-write is one `Immediate` transaction *and* the
  `UPDATE` repeats `AND lease_token_hash = ?3`, so fencing holds even against a
  stale read.

**Locking model, honestly:** in-process serialization is complete (one mutex);
cross-process there is no store-level lock at all. Mutual exclusion between a
running node and the CLI comes from the separate `<db>.lock` file (`fs2`
`try_lock_exclusive`), and WAL mode is what lets the CLI's read-only connections
proceed without blocking the node's writer.

**Durability assumptions:** `journal_mode=WAL` + `synchronous=FULL` means each
commit fsyncs the WAL before returning. WAL creates `executions.sqlite-wal` and
`executions.sqlite-shm` sidecars; `operations::file_family_bytes` sums all three
for `execution_bytes`. No `wal_autocheckpoint` tuning, no `VACUUM`, and no WAL
truncation — the file grows monotonically and checkpoints on SQLite's default
policy. `PRAGMA foreign_keys=ON` is per-connection and reapplied on every
`open`; the CLI's `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX` connections read
FK state but do not need it.

## Bounded-ness

Every limit the module enforces, with values. Rows marked *core* are enforced by
`eggwork-core` but reached through this store.

| Bound | Value | What it bounds / where enforced |
|---|---|---|
| `MAX_RETAINED_EVENTS` | `256` | Events retained per `(execution_id, generation)`; `trim_events` |
| `MAX_RETAINED_EVENT_BYTES` | `eggwork_core::MAX_EVENT_BYTES` (`512 * 1024`) | (a) retained bytes per generation, in `trim_events`; (b) per-event ceiling in `commit_event` (`encoded.len() > MAX_RETAINED_EVENT_BYTES` -> `InvalidEvent`). The same constant does double duty. (b) is **not** implied by the 64 KiB core payload cap: `Stdout`/`Stderr` serialize as a JSON number array at up to four characters per byte, so a maximum-size chunk encodes to ~256 KiB. It is the shared ceiling the client also reads — see `client-transport.md` — so the two sides agree by construction |
| `MAX_EXECUTION_IDENTITIES` | `2048` | `COUNT(*) FROM executions` - i.e. *rows*, so it counts *(id, generation)* pairs, not distinct `execution_id`s, despite the name. Checked only on the new-row path in `reserve`; a generation bump consumes budget |
| `MAX_EVENT_CHUNK_BYTES` *(core)* | `64 * 1024` | The real per-event payload ceiling, via `ExecutionEvent::validate` at `store.rs:319` |
| `MAX_ID_BYTES` *(core)* | `128` | `ExecutionId` / `LeaseId` / `PrincipalId`, upstream of the store |
| metric name length | `1..=64` bytes | `increment_metric` / `increment_metrics` (`store.rs`) |
| metric value | saturating at `i64::MAX` | `CHECK(value >= 0)` + the upsert `CASE` |
| `load_page` result size | <= 256 implicitly | **no SQL `LIMIT`**; bounded only by trimming |


Not bounded — review concerns:

- **`next_sequence` is unbounded.** A long execution emitting millions of stdout
  chunks keeps incrementing it forever (retained rows stay ≤ 256). `i64`, so
  overflow is not realistic, but there is no cap and no wrap handling.
- **`snapshot_json` size is unchecked by this module** (only event encodings are
  length-checked); it is bounded only implicitly by `ExecutionResult`'s
  core-side caps.
- **`MAX_EXECUTION_IDENTITIES` never evicts.** The closure states this
  deliberately — "the store does not evict identities and thereby preserves
  idempotency" — but the effect is a permanent 503 `storage_exhausted` after
  2048 identities, with no age- or count-based reclamation and no operator
  remedy. `recover()` reclaims nothing.
- **`COUNT(*)` is a full scan** per new-identity `reserve`; fine at ≤ 2048 rows.
- **No cap on `node_metrics` row count** beyond the 64-byte name bound, and **no
  bound** on open read-only CLI connections or on how long the single `Mutex` may
  be held.

## Error model

`StoreError` (`store.rs:21-31`) — four variants, all mapped by callers to a
generic 500 `ServiceError::internal("execution store unavailable")` except where
noted:

| Variant | Display | Source | Retryable? |
|---|---|---|---|
| `Sql(#[from] rusqlite::Error)` | `execution store failed` | Any SQLite error: `SQLITE_BUSY` after the 2 s busy timeout, `SQLITE_CONSTRAINT_PRIMARYKEY` / `SQLITE_CONSTRAINT_CHECK`, `SQLITE_IOERR`, disk full, corrupt database, and (at `store.rs:73-75`) the file-creation `std::io::Error` wrapped as `rusqlite::Error::ToSqlConversionFailure` | Transient only for `Busy`/`BusyTimeout`/recoverable `Io`. There is no retry anywhere in `store.rs`, so "retryable" is left to the caller, which does not retry. |
| `Json(#[from] serde_json::Error)` | `execution store serialization failed` | Encoding a snapshot/event, or **decoding** a stored blob. A decode failure here means on-disk data is unreadable for this code version — not retryable, and it is indistinguishable from corruption. | No |
| `Worker` | `execution store worker failed` | Two distinct causes collapsed into one: `connection.lock()` poisoning (`PoisonError`) and `spawn_blocking` `JoinError` (panic or runtime shutdown). `store.rs:131, 212` and the equivalent `.map_err(|_| StoreError::Worker)` on every `await` | Poisoning is a programming/panic bug; join failure is terminal |
| `InvalidEvent` | `execution event exceeds its protocol bound` | `event.validate()` failure, `encoded.len() > MAX_RETAINED_EVENT_BYTES`, **and** an invalid `increment_metric` name. The metric-name reuse is a misnomer: a bad counter name is not an event problem. | No |

**Constraint violation vs. I/O failure.** Both arrive through the same
`StoreError::Sql`, but they are distinguishable by the inner `rusqlite::Error`
enum, which this module does not pattern-match on:

- `rusqlite::Error::SqliteFailure(error, _)` where `error.extended_code` is
  `SQLITE_CONSTRAINT_PRIMARYKEY` (a duplicate `(id, generation)` insert) or
  `SQLITE_CONSTRAINT_CHECK` (a negative `node_metrics` value) — a *logical*
  rejection. With the current code these should be unreachable: the `Immediate`
  transaction plus the read-before-write means `reserve` returns `Existing`
  before it can insert a duplicate, and `increment_metric`'s `CASE` makes a
  negative value impossible. They are latent invariants, not handled paths.
- `rusqlite::Error::SqliteFailure` with `SQLITE_BUSY`/`SQLITE_LOCKED` -
  *contention*, resolved by the 2 s `busy_timeout`.
- `rusqlite::Error::SqliteFailure` with `SQLITE_IOERR` / `SQLITE_FULL` /
  `SQLITE_CORRUPT` / `SQLITE_NOTADB` - genuine I/O. Disk-full on `commit_event`
  is the dangerous case: it makes the node drain and cancel (`lib.rs:2590-2593`)
  but the transition is not persisted, so the execution stays nonterminal durably
  and is interrupted on the next boot.

The closure explicitly records: "No direct test forced SQLite IO failure/disk-full
behavior. Code inspection confirms state is published in memory only after
durable commit and failure drains/cancels the node."


## Drain state and the exclusive lock

Drain is **not** a row in the execution database. It is a filesystem sentinel
sibling to it:

- Path: `config.database_path.with_extension("drain")` — for the default
  `state/executions.sqlite` that is `state/executions.drain`
  (`lib.rs:347`; the same derivation is exposed as
  `OperatorConfig::drain_path()`, `operations.rs:327`).
- Written by `operations::set_persistent_drain` (`operations.rs:533-565`):
  `create_new` a temp file `executions.drain.<uuid>.tmp` with mode `0600`,
  write the literal `b"draining\n"`, `sync_all`, `rename` over the target, then
  `fsync` the parent directory on Unix. Clearing is `remove_file` (ignoring
  `NotFound`) plus a parent `sync_all`. The rename-then-fsync ordering means a
  reader never sees a half-written marker.
- Read by `operations::is_persistently_draining` (`operations.rs:568-570`):
  `fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())`.
  **Contents are never read** — presence is the whole signal. Using
  `symlink_metadata` means a symlink to a file reports `is_file() == false` and
  is therefore *not* treated as draining; the same anti-symlink posture as
  `operations::require_regular_file`, which refuses a symlinked database.

A running node consults it **twice per submit**, both before admission:
`execute` checks `state.draining.load(Ordering::Acquire) ||
operations::is_persistently_draining(&state.drain_path)` at `lib.rs:1569-1571`
(-> 503 `draining`, `rejected_draining`) and repeats it at `lib.rs:1702-1711`
*after* the `lookup` and after taking the in-memory executions mutex, closing the
window where a concurrent `set_draining(true)` lands mid-request. The in-memory
`Arc<AtomicBool>` is the fast path; the file is the durable cross-restart path,
seeded at startup (`lib.rs:346-348`).

**The exclusive state lock.** `acquire_state_lock` (`lib.rs:540-556`) opens
`database_path.with_extension("lock")` - `state/executions.lock` - with mode
`0600` and calls `fs2::FileExt::try_lock_exclusive`. `NodeServer::start` calls it
**before** `ExecutionStore::open` (`lib.rs:315-317`) and holds the `File` as
`NodeServer::_state_lock` for the node's lifetime. Therefore:

- A second `NodeServer::start` on the same path fails with
  `NodeStartError::AlreadyRunning` ("another node process already owns this state
  directory") before any SQLite handle exists.
- `operations::collect_garbage` with `dry_run == false` (`operations.rs:813`, via
  `acquire_maintenance_lock` at `operations.rs:860-875`) takes the **same**
  `try_lock_exclusive` and so fails with `OperationsError::Data` while a node is
  running - deliberate, since CLI GC would otherwise mutate workspace, artifact
  and blob state under a live node. `dry_run_gc` skips the lock because it opens
  only `SQLITE_OPEN_READ_ONLY` connections.
- Contrast the **live** `NodeServer::collect_garbage` (`lib.rs:460-508`), which
  takes no file lock at all: it uses the already-open `WorkspaceManager`,
  `BlobStore`, and `ArtifactStore` handles inside the owning process, where the
  single store mutex already guarantees exclusion, and it clamps its batch to
  `requested_limit.clamp(1, 1024)`.
- Read-only CLI commands (`execution_page`, `execution_show`,
  `active_execution_count`, `metrics_snapshot`) take **no** lock, relying on
  `require_regular_file` (no symlink) plus
  `open_with_flags(SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX)`; WAL guarantees
  they neither block nor are blocked by the node's writer.

The lock protects the *state directory* against a second node, not against a
third-party process that ignores the convention, and it does not authenticate the
holder.

## Metrics

`increment_metric` is a thin `let _ = store.increment_metric(name, amount).await;`
wrapper, and `increment_metrics` its batched sibling — every metric failure is
silently discarded. The `name: &'static str` argument means metric names are
compile-time constants chosen by `lib.rs`, never request-derived, so the 64-byte
name bound is defense in depth. Because `commit_event` needs the *same* connection
mutex, unbatched metric traffic queued in front of durable event commits: the
execution-terminal path used to await up to four sequential increments, and
`create_workspace_derived` three. `increment_metrics` takes the deltas as a slice
and applies them in one `spawn_blocking`, one connection acquisition and one
explicit transaction, with the per-name upsert arithmetic unchanged; both those
sites now use it. So a terminal transition costs one metric write instead of up
to four — batching amortises the mutex and the commit, not the number of upserts.

The 20 names in `operations::METRIC_NAMES` (`operations.rs:28-50`) that flow
through this store:

| Name | Incremented at | Measures |
|---|---|---|
| `executions_accepted` | `lib.rs:1791` | New execution identities durably reserved (`ReserveResult::Created`) |
| `rejected_route` | `lib.rs:729` | Requests with no matching route |
| `rejected_unauthenticated` | `lib.rs:733` | Requests without a resolvable verified principal |
| `rejected_unauthorized` | `lib.rs:748` | Principal failed operation/resource authorization |
| `rejected_invalid` | `lib.rs:1051, 1172, 1545, 1554, 1562, 1609, 1624, 1651, 1749` | Malformed spec, bad schema version, unusable workspace, runner-request build failure |
| `rejected_draining` | `lib.rs:1572, 1705` | Submit refused because the node is draining (both checks) |
| `rejected_capability` | `lib.rs:1580, 1588, 1596` | Isolation / resource / network requirement unavailable |
| `rejected_busy` | `lib.rs:1715` | `max_active_executions` semaphore exhausted |
| `rejected_storage` | `lib.rs:1734, 1837` | In-memory `MAX_RECENT_EXECUTIONS` (1024) or durable `MAX_EXECUTION_IDENTITIES` (2048) |
| `event_history_resync` | `lib.rs:2326` | Client cursor fell behind `base_sequence` → 410 |
| `stdout_bytes` / `stderr_bytes` | `lib.rs:2499-2500` | Bytes the runner reported as produced, by `execution_result` (not journal bytes) |
| `terminal_succeeded` / `terminal_failed` / `terminal_cancelled` / `terminal_timed_out` / `terminal_interrupted` | `lib.rs:2488-2498` (by outcome) and `lib.rs:331` (by `recover()` count) | Terminal outcomes; `terminal_interrupted` is the only one incremented at startup |
| `cleanup_failures` | `lib.rs:2502` | Terminal results carrying a `cleanup_warning` |
| `blob_upload_bytes` / `blob_download_bytes` | `lib.rs:1296, 1324` | Data-plane byte counters that happen to live in this node DB |

Gap worth knowing: five names the server writes — `workspace_manifest_
registrations`, `workspace_derived_requests`, `workspace_derived_hits`,
`workspace_patch_entries`, `workspace_base_missing` (`lib.rs:1070, 1130, 1192-1208`)
— are **not** in `METRIC_NAMES`, so `operations::metrics_snapshot`
(`operations.rs:781`, `SELECT name,value FROM node_metrics`) silently discards
them: it seeds only the fixed list and skips unknown names. They accumulate
correctly in the table and are simply invisible to the CLI.

Also note `metrics_snapshot` reads the live database read-only while a node may
be writing: WAL makes that consistent per-statement, but the 20 counters are
individually atomic, so a multi-row snapshot can straddle a commit.

## Invariants and enforcement

| Invariant | Enforcement site | Test |
|---|---|---|
| One row per `(execution_id, generation)` | `PRIMARY KEY (execution_id, generation)` | implicit; `duplicate_identity_is_idempotent_and_conflicting_digest_is_rejected` |
| Only generation 1 may start an identity | `reserve`, `None if generation != 1 => StaleGeneration` (`store.rs:167`) | not directly unit-tested; `StaleGeneration` reachable via `execute`'s 409 `generation_mismatch` |
| Generations advance by exactly one from a terminal predecessor | `reserve`, `generation != latest_generation + 1 \|\| !matches!(latest_state, "Succeeded"\|…)` (`store.rs:169-176`) | server test `mtls_authorization_and_fixed_target_execution` (stale-generation cancel → 409) |
| Same `(id, gen)` + same `(version, digest, principal, lease hash)` is idempotent | `reserve` → `ReserveResult::Existing` (`store.rs:145-150`) | `duplicate_identity_is_idempotent_and_conflicting_digest_is_rejected`; 24-way concurrent submit in `mtls_authorization_and_fixed_target_execution` |
| Different digest never mutates the existing execution | `reserve` → `ReserveResult::Conflict` (`store.rs:152`); `execute` → 409 | `duplicate_identity_is_idempotent_and_conflicting_digest_is_rejected`; same 409 assertion in the mTLS test |
| A principal cannot observe another principal's execution | `load_snapshot_for_principal` `AND principal_id = ?3` (`store.rs:263-274`) | `snapshot_lookup_is_principal_fenced_and_hides_foreign_execution`; `cross_principal_execution_fencing_without_lease_oracle` |
| Raw lease token is never persisted | `lease_hash` = SHA-256 hex; only `lease_token_hash` is a column | none in-file; asserted structurally by `cross_principal_execution_fencing_without_lease_oracle` |
| Only the lease holder may renew or cancel | `update_lease` `current_hash != lease_token_hash => None` plus `UPDATE … AND lease_token_hash = ?3`; `control` compares `existing.lease_token_hash` and `record.lease_hash` | `cross_principal_execution_fencing_without_lease_oracle` |
| Renewal is idempotent and never double-extends | `last_renewal_id == Some(renewal_id) => return current_expiry`; `expires = current_expiry.max(requested)` (`store.rs:383-387`) | `lease_renewal_replay_is_idempotent` |
| An expired lease cannot be renewed | `current_expiry <= unix_millis() => None` (`store.rs:378`) | `expired_lease_terminates_process_with_a_typed_terminal_result`; in-memory mirror at `lib.rs:2117-2124, 2154-2161` |
| Terminal state is single-assignment | `commit_event` `is_terminal_name(&state) => rollback, Ok(None)`; `publish`'s in-memory `is_terminal` early return | repeated-cancel case in the lifecycle integration test (one `Cancelling`, one `Cancelled`) |
| State and its event commit atomically | one `Immediate` transaction around `UPDATE` + `INSERT` (`store.rs:294-344`) | `recovery_marks_uncertain_work_interrupted_and_never_replays_it` (asserts the appended event and the advanced sequence) |
| Event sequences are gap-free within a generation | read `next_sequence` + write `next_sequence = sequence + 1` in one `Immediate` transaction; `PRIMARY KEY (id, gen, sequence)` | `recovery_...` asserts `events[2].sequence.get() == 3`; `event_journal_trims_...` asserts the retained head equals `base_sequence` |
| Events are scoped to a generation | `load_page` filters on `execution_id` **and** `generation` | `event_journal_trims_by_count_and_bytes_with_explicit_base_cursor` |
| Retention is bounded by count and bytes | `trim_events` loop after each insert (`store.rs:607-630`) | `event_journal_trims_by_count_and_bytes_with_explicit_base_cursor` (300 × 2 KiB events) |
| A single event cannot exceed the byte ceiling | `encoded.len() > MAX_RETAINED_EVENT_BYTES` → `InvalidEvent` (`store.rs:321-323`); payload cap from `ExecutionEvent::validate` | not directly asserted in-file |
| An expired cursor gets no partial replay | `load_page` returns empty `events` with the true `base_sequence`; `event_response` maps it to 410 | `event_journal_trims_...` ("expired cursor returns no partial replay") |
| Identities are capped | `execution_count >= MAX_EXECUTION_IDENTITIES => StorageFull` (`store.rs:181-185`) | none — no test drives the 2048 cap |
| Recovery interrupts, never replays | `recover()` sets `Interrupted` + `ExecutionFailure::Interrupted`, spawns nothing | `recovery_marks_uncertain_work_interrupted_and_never_replays_it` (asserts idempotent second pass); `restart_recovers_uncertain_execution_as_interrupted_without_replay` |
| One node process owns the state directory | `acquire_state_lock` `try_lock_exclusive` before `ExecutionStore::open` | `operator_drain_persists_across_node_restart` asserts `AlreadyRunning` |
| Drain is durable across restart | `set_persistent_drain` atomic rename + dir fsync; `is_persistently_draining` existence check | `operator_drain_persists_across_node_restart`; `drain_rejects_new_admission_while_terminal_records_survive`; `concurrent_drain_admission_outcomes_are_closed` |

## Test coverage

In-file (`store.rs` `mod tests` - 5 `#[tokio::test]`s, each on a fresh
`TempDir` database):

| Test | Asserts |
|---|---|
| `snapshot_lookup_is_principal_fenced_and_hides_foreign_execution` | generation-pinned lookup for a foreign principal returns `None`; unpinned lookup for the owner returns the snapshot |
| `recovery_marks_uncertain_work_interrupted_and_never_replays_it` | after reserving + committing `Accepted` and `Running`, dropping and reopening: `recover()` returns one `Interrupted` snapshot with `ExecutionFailure::Interrupted`, the journal holds 3 events with the third at sequence 3, and a second `recover()` is empty |
| `event_journal_trims_by_count_and_bytes_with_explicit_base_cursor` | 300 x ~2 KiB `Diagnostic` events leave `base_sequence > 1`; cursor 0 returns an empty page ("expired cursor returns no partial replay"); a cursor at `base_sequence - 1` returns the retained window starting exactly at `base_sequence`, with `events.len() <= MAX_RETAINED_EVENTS` and re-encoded bytes `<= MAX_RETAINED_EVENT_BYTES` |
| `duplicate_identity_is_idempotent_and_conflicting_digest_is_rejected` | identical re-`reserve` -> `Existing`; changed digest -> `Conflict` |
| `lease_renewal_replay_is_idempotent` | the same `renewal_id` replayed with a *later* requested expiry returns the identical persisted deadline |


Server-level tests that exercise this store (in `lib.rs` `mod tests`):

- `mtls_authorization_and_fixed_target_execution` — the 24-way
  `join_all` of identical `execute` calls (line ~3831) against one handle, with
  a filesystem `spawn-count` marker asserted to contain exactly `"x"`, then a
  changed spec on the same handle asserted to be 409. This is the ADR-0003
  "N concurrent duplicate submissions create one child process" proof.
- `expired_lease_terminates_process_with_a_typed_terminal_result` — lease expiry
  cancels the child and yields a typed terminal result.
- `restart_recovers_uncertain_execution_as_interrupted_without_replay` — seeds a
  durable `Running` row directly through `ExecutionStore::reserve` /
  `commit_event`, starts a node on the same DB, observes `Interrupted` with zero
  active children, and reads the recovery event.
- `cross_principal_execution_fencing_without_lease_oracle` — two principals, one
  execution, no lease-derived information leak.
- `drain_rejects_new_admission_while_terminal_records_survive` and
  `concurrent_drain_admission_outcomes_are_closed` — drain admission races.
- `operator_drain_persists_across_node_restart` — drain marker survives a
  restart and `AlreadyRunning` is returned for a concurrent `start`.
- `mtls_execution_streams_output_and_missing_certificate_is_rejected` — durable
  event replay over the stream.
- `lease_expiry_takes_precedence_over_cancel_before_spawn` — the
  `failure_for_runner_error` precedence correction recorded in
  `plans/closure/control-plane-protocol/002-lease-expiry-race-correction.md`.

Honest gaps:

- **No concurrent-submit test at the store level.** The 24-way race is proven
  end-to-end through HTTP; `store.rs` has no test issuing two `reserve` calls
  concurrently, so the mutex + `Immediate` + PK argument is argued from code,
  not asserted.
- **`MAX_EXECUTION_IDENTITIES` (2048) is never exercised** — no test drives
  `StorageFull`; and `StaleGeneration` is never asserted directly (neither the
  "generation must be 1" nor the "latest+1 from a terminal predecessor" branch).
- **No SQLite I/O failure / disk-full test.** The drain-and-cancel fallback in
  `publish` on `Err(_)` is untested, as the closure states. There is also no
  torn-read test for `load_page`'s non-transactional three-statement read and no
  `trim_events` performance check for one append evicting many rows.
- **Renewal-at-the-expiry-boundary race is explicitly not stress-tested** — the
  closure calls it "an unqualified verification gap"; cancel-versus-natural-exit
  is likewise recorded as untested, as are the accepted-before-spawn and
  post-terminal-persist crash cut points (each unexercised as a separate
  daemon-kill scenario).
- **`load_all` is only covered via the startup seeding path.**

## Review focus

- **Idempotency under concurrent duplicate submit.** The guarantee rests on
  (a) one connection behind a `std::sync::Mutex`, (b) `TransactionBehavior::
  Immediate`, (c) the composite PK, and (d) the exclusive `.lock` file. Only the
  end-to-end HTTP path is tested. If `ExecutionStore` were ever cloned into a
  second process, or the `lookup` fast path in `execute` were trusted instead of
  `reserve`, the reasoning changes. That `lookup`+`reserve` double-check is
  itself redundant and only safe because `acquire_state_lock` admits one node
  process — confirm that invariant is never relaxed.
- **The state machine is not enforced by the store.** `commit_event` accepts any
  non-terminal → non-terminal transition; all legality lives in `publish` and
  its callers, so a new call site could persist an illegal transition. Compounding
  this, the nonterminal/terminal sets are spelled out in at least four places —
  `store::is_terminal_name`, `recover()`'s SQL `IN` list, `lib.rs::is_terminal`,
  and the `operations.rs` active filters — with nothing enforcing agreement when
  a state variant is added.
- **Unchecked integer narrowing.** `ExecutionGeneration::new` only rejects `0`
  (`eggwork-core/src/lib.rs:113-120`), yet the store does
  `snapshot.generation.get() as i64` in **eleven** places (an earlier revision of this
  document said six); a generation above
  `i64::MAX` wraps negative and would be written to the PK. Same class of issue
  for `after_sequence as i64` in `load_page`, guarded only at the HTTP route and
  not in the store method itself. `sequence + 1` and the `.min(i64::MAX as u128)`
  clamps in `unix_millis` / `lease_ttl` conversions are currently safe, but
  `next_sequence` is genuinely unbounded over a long execution.
- **Lease fencing correctness.** `update_lease` is fenced at both the read and
  the write. But `control` (`lib.rs:2086-2092`) *additionally* compares the
  in-memory `record.lease_hash`, and records loaded at startup get
  `lease_hash: String::new()` (`lib.rs:369`), as does `events_route`'s
  `recovered_record` (`lib.rs:2297`) — a recovered record's in-memory lease hash
  is empty by construction, so any future control path relying on that field for
  a recovered execution would be wrong. Separately, a *different* `renewal_id`
  with an earlier requested expiry is silently clamped by
  `current_expiry.max(...)` rather than rejected, so a client cannot distinguish
  "renewed" from "clamped".
- **No schema version or migration path.** `CREATE TABLE IF NOT EXISTS` plus a
  snapshot-level `schema_version` that is never checked. Any future column
  addition is an unversioned breaking change, and a node cannot detect an old
  database and refuse cleanly.
- **Durability assumptions are unverified.** `synchronous=FULL` + WAL, no
  explicit checkpointing, no `VACUUM`, unbounded WAL growth, three sidecar files
  on disk — and no test forces I/O failure or disk-full.
- **`load_page` has no `LIMIT` and is not a consistent read** (three separate
  autocommit statements), bounded today only because trimming caps retained rows
  at 256. `trim_events` is O(evicted rows) inside the caller's transaction, so a
  single append can hold the write lock and the process-wide mutex for many
  deletes.
- **`MAX_EXECUTION_IDENTITIES` is a one-way door.** 2048 is the node's lifetime;
  the closure is explicit that no eviction happens (to preserve idempotency), but
  the degradation mode is an unexplained 503 with no operator remedy. The
  constant also counts *(id, generation)* rows, not identities.
- **Error-model and constant hygiene.** `StoreError::InvalidEvent` is reused for
  a bad metric name (`store.rs:484-486`), so metric failures and event bound
  violations are indistinguishable at the type level;
  `MAX_RETAINED_EVENT_BYTES` does double duty as both a per-event ceiling and a
  per-generation retention ceiling, with `MAX_EVENT_CHUNK_BYTES` (64 KiB) as the
  actually-binding per-event limit.
- **Observability gap.** Five `workspace_*` counters are written through this
  store but absent from `METRIC_NAMES`, so `operations::metrics_snapshot` reads
  and discards them.
- **No SQL injection surface.** Every statement is a literal with `?n`
  placeholders and `params!`/array bindings; there is no dynamic SQL
  construction anywhere in `store.rs`, both `ORDER BY` clauses are fixed, and
  `operations.rs` parameterizes everything it reads. This is a clean area.

## Related

- [architecture overview](overview.md)
- [core domain](core-domain.md)
- [server node](server-node.md)
- [data plane](data-plane.md)
- [operations CLI](operations-cli.md)
- [protocol](protocol.md)
- [ADR-0003: Execution Idempotency, Ownership Leases, and Generation Fencing](../plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md)
- [M002 plan: Idempotency, Leases, Resumable Events, and Recovery](../plans/implementation/control-plane-protocol/002-idempotency-leases-events-and-recovery.md)
- [M002 closure evidence](../plans/closure/control-plane-protocol/002-status.md)
- [M002 corrective closure: lease expiry before runner spawn](../plans/closure/control-plane-protocol/002-lease-expiry-race-correction.md)
- [Long-term specification §8, §10, §18](../plans/000-long-term-specification.md)

`server-node.md`, `data-plane.md`, and `operations-cli.md` are the companion
deep dives for `lib.rs`, the data-plane stores, and `operations.rs`
respectively.
