# Control Plane M002 — Idempotency, Leases, Resumable Events, and Recovery

Status: blocked on Control Plane M001 closure

Source roadmap:

- `plans/subsystems/control-plane-protocol-roadmap.md`

Canonical/ADR references:

- `plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md`
- `plans/000-long-term-specification.md#18-persistence-and-restart-recovery`

## 1. Objective

Make accepted execution safe under duplicate requests, ambiguous transport failure, reconnect, stale controllers, lease expiry, and eggworkd restart.

## 2. Durable storage

Introduce a small node-owned durable store, likely SQLite unless implementation review identifies a simpler equally robust local mechanism.

Persist before spawn enough state to prove:

- execution ID;
- generation;
- request digest;
- normalized accepted request/reference;
- principal/authorization attribution reference;
- lifecycle state;
- lease ID/expiry;
- event sequence/base retention sequence;
- timestamps;
- terminal result summary;
- workspace/artifact references when available.

Never persist unredacted secret-bearing environment values merely for diagnostics.

## 3. Request canonicalization and digest

Define a versioned canonicalization algorithm.

The digest must be stable for semantically identical accepted requests and must not depend on JSON object field order.

Record canonicalization version.

Same ID/generation/digest returns existing execution.
Same ID/generation/different digest returns typed conflict.

## 4. Generations and fencing

Define exact transitions for:

- first generation;
- renewal within generation;
- optional controller takeover/new generation;
- stale get versus stale mutation;
- terminal execution generation advancement if permitted.

All control mutations verify current generation.

## 5. Leases

- accepted live execution gets a lease;
- renew is idempotent;
- expiry uses monotonic runtime timing plus persisted wall-time evidence needed for restart;
- default orphan policy terminates;
- detached mode remains disabled unless separately authorized/configured;
- cancellation is distinct from expiry.

## 6. Event journal/retention

Create a bounded sequence ring/journal with:

- monotonically increasing sequence;
- bounded per-event size;
- bounded per-execution retained bytes/events;
- base_seq and next_seq;
- resume after sequence;
- typed HistoryExpired/CursorAhead/GenerationMismatch/HandleGone-style resync reasons;
- terminal snapshot.

Do not retain unbounded stdout in the database. Content may be ring/file/artifact backed.

## 7. Restart recovery

At startup:

- reconcile nonterminal executions;
- prove whether a child can still be owned/adopted; initial implementation may conservatively mark uncertain work Interrupted;
- do not rerun automatically;
- expire/reconcile leases;
- preserve terminal records;
- verify workspace/artifact references;
- emit recovery evidence without fabricating process exit.

## 8. Required race tests

- 20+ concurrent identical submissions -> one child;
- conflicting digest race;
- duplicate cancel;
- cancel vs exit;
- renew vs expiry;
- stale generation cancel/renew;
- stream reconnect within retention;
- reconnect before base sequence;
- cursor ahead;
- restart after accept before spawn;
- restart while running;
- restart after child exit before terminal persist;
- restart after terminal persist;
- shutdown during cleanup.

## 9. Acceptance criteria

1. Network retry cannot duplicate one accepted execution.
2. Identity collision cannot mutate existing execution.
3. Stale generation cannot control newer state.
4. Lease semantics are deterministic across disconnect/restart.
5. Event resume is continuous or explicitly resynchronized.
6. Restart never blindly re-executes uncertain work.
7. Terminal state is single-assignment.
8. Storage/output retention is bounded.

## 10. Stop conditions

Stop if:

- canonical digest would include persisted plaintext secrets;
- restart safety would require auto-replaying arbitrary commands;
- lease implementation depends on one live TCP connection;
- event resume can only be implemented with unbounded history.

## 11. Closure evidence

Create `plans/closure/control-plane-protocol/002-status.md` with race/restart matrix, storage schema, canonicalization version, lease policy, event retention limits, and exact recovery evidence.
