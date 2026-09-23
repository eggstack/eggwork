# Control Plane M002 Closure — Idempotency, Leases, Resumable Events, and Recovery

Source plan: `plans/implementation/control-plane-protocol/002-idempotency-leases-events-and-recovery.md`  
Subsystem roadmap: `plans/subsystems/control-plane-protocol-roadmap.md`  
Reviewed implementation commit: `83600ec`  
Planning/closure commit: `9b34717`

## Finding

M002 is closed for the exercised Linux host. The node now persists identity reservations, lifecycle snapshots, lease evidence, and a bounded sequenced event journal in SQLite before process spawn. The client sends a stable execution handle on execute and every control call. A matching retry returns the existing execution; changed request/lease identity conflicts. New generations advance only from the latest terminal generation, and control mutations are fenced to the latest generation. Daemon startup marks uncertain nonterminal work Interrupted and does not rerun it. Event replay uses durable sequence cursors and typed history-expired/cursor-ahead responses.

The implementation uses `eggserve-core 0.2.0` and keeps transport ownership in EggServe/Eggfetch. No scheduler, automatic retry, detached execution mode, or process adoption was added.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Durable reservation before spawn | `ExecutionStore::reserve` transaction records execution ID, generation, canonical version/digest, principal, hashed lease ID, wall-clock lease expiry, state, sequence cursor, JSON snapshot, and timestamps. SQLite is configured for WAL, `synchronous=FULL`, and foreign keys; newly created DB files use mode `0600` on Unix. `execute` reserves before spawning the runner. |
| Canonical digest independent of JSON field order | `eggwork-core::request_digest` sorts unordered environment, metadata, and declared-output fields and hashes a versioned typed JSON representation with the `canonical-json-v2` domain prefix. Duplicate environment names are rejected. Core tests compare reordered inputs and confirm duplicate-name rejection. Only the digest/version are stored; normalized environment plaintext is not persisted for diagnostics. |
| Idempotency and identity collision | Store reservation is transactional. The loopback test launches 24 identical concurrent submissions for one handle and checks a filesystem marker was written once. Reusing the identity with a changed spec returns 409. Reservation also conflicts on principal or hashed lease mismatch. |
| Generations and fencing | Generation 1 is the only valid initial generation; a new generation must be exactly latest+1 and the prior generation terminal. The integration test advances to generation 2 and verifies stale generation 1 cancellation gets 409. Cancel and renew also validate principal, lease hash, and latest generation before mutation. |
| Lease policy and disconnect semantics | Accepted work gets a node-side monotonic deadline; renewal updates durable wall-clock expiry and monotonic runtime expiry. Replaying the same renewal ID returns the persisted deadline without extending twice. A separate integration test verifies lease expiry cancels the child and returns `Interrupted/LeaseExpired`; dropping an event stream leaves execution running. Lease expiry is terminate-on-orphan; detached mode is not implemented. |
| Bounded event journal and resume | Each execution generation retains at most 256 events and 512 KiB; an event above the byte ceiling is rejected. The storage test pushes 300 large events and verifies trimming/base-cursor behavior and the retained-byte bound. HTTP event replay returns typed 410 for a cursor before retained history and 416 for a cursor ahead of `next_sequence`; replay ends after terminal state. The live broadcast remains bounded at 32 listeners/events and lagging consumers get a stream error. |
| Restart recovery without replay | Store recovery changes each persisted nonterminal state (`Accepted`, `Preparing`, `Running`, or `Cancelling`) to `Interrupted`, persists a typed interruption result and terminal event, and is idempotent on a second recovery. The server restart test seeds durable running state, starts a new node on the same DB, observes Interrupted with zero active children, and reads the recovery event. No command is reconstructed or spawned. |
| Terminal single-assignment and persistence failure | Store event commit validates monotonic sequence and refuses mutation after terminal state. In-memory state changes only after durable commit; a failed commit drains the node and cancels that execution. The lifecycle integration test covers repeated cancel and confirms exactly one Cancelling and one Cancelled event. |
| Bounded durable identity retention | Execution identities/generations have a lifetime cap of 2048; reaching it refuses new identities with typed storage exhaustion. The store does not evict identities and thereby preserves idempotency. Event retention is separately bounded as above. |

## Verification actually run

Toolchain: repository Rust 1.89.0.

- `cargo fmt --all` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed (26 tests across 4 suites; RTK summary, 5.30s).
- `cargo tree -p eggwork-server -i eggserve-core` — confirmed EggServe 0.2.0.
- `git diff --check` — passed before implementation commit.

## Compatibility, security, and platform review

- Only Linux loopback/process execution was exercised. Other Unix platforms and Windows are not qualified by this closure.
- The durable database stores the request digest and principal attribution, not the full request or plaintext environment. The lease credential is stored only as a SHA-256 hash. API errors remain fixed and bounded.
- Event history is bounded and old cursors require explicit resynchronization. There is no cross-node event log.
- Recovery was executed against a running-state fixture. The recovery implementation handles all persisted nonterminal state names; process ownership/adoption is deliberately not attempted. The accepted-before-spawn and post-terminal crash windows were not each exercised as separate daemon-kill integration scenarios.
- The exercised race coverage includes 24 concurrent same-identity submissions, identity conflict, repeated cancellation, stale-generation cancellation, renewal replay, and expiry. A simultaneous cancel-versus-natural-exit race and a renewal-at-the-expiry-boundary race were not separately stress-tested; these remain residual test gaps, not known implementation failures.
- No direct test forced SQLite IO failure/disk-full behavior. Code inspection confirms state is published in memory only after durable commit and failure drains/cancels the node.
- Lease wall-clock expiry is persisted for evidence and renewal idempotency. On restart, every uncertain nonterminal execution is interrupted regardless of the remaining wall-clock lease; this conservative policy avoids re-executing commands.
- No high/medium finding blocks closure. Eggress route support remains Control Plane M003 and is blocked on confirming current public Eggfetch/Eggress dial compatibility.
- Disposition: **closed for the exercised Linux host**, with the unexercised races/restart cut points above recorded as residual verification gaps.

## Registry/roadmap and next-plan disposition

Control Plane M002 is closed. Workspace M001's only hard dependency is satisfied and it is promoted to active. Security M001 remains ready, as its Foundation M001 and Control Plane M001 dependencies are satisfied; it remains later in the requested sequence. Workspace M002/M003, Security M002/M003, Operations M001/M002, and CodeGG M001 remain blocked by their outstanding direct dependencies. Control Plane M003 remains blocked only on public Eggfetch/Eggress route-interface verification.
