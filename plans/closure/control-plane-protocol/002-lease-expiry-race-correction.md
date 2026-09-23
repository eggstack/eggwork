# Control Plane M002 Corrective Closure — Lease Expiry Before Runner Spawn

Historical closure: `plans/closure/control-plane-protocol/002-status.md`  
Source plan: `plans/implementation/control-plane-protocol/002-idempotency-leases-events-and-recovery.md`  
Reviewed corrective commit: `428e2ce`  
Planning/corrective closure commit: pending

## Historical claim requiring qualification

The original M002 closure described an expired lease as producing `Interrupted/LeaseExpired`. A later full-suite run exposed an ordering race when the lease deadline elapsed before the runner spawned: the runner returned `CancelledBeforeSpawn`, which was classified as generic `Interrupted` before the expired-lease check. The historical record is retained unchanged; this addendum is the current qualification.

## Correction and evidence

`failure_for_runner_error` now gives spawn failure precedence, then classifies an elapsed lease as `LeaseExpired`, then classifies a non-expiry cancellation-before-spawn as `Interrupted`. A deterministic unit test, `lease_expiry_takes_precedence_over_cancel_before_spawn`, covers the precedence. The existing integration test `expired_lease_terminates_process_with_a_typed_terminal_result` covers lease expiry through the node lifecycle.

Verification after the correction:

- Targeted `cargo test -p eggwork-server expired_lease_terminates_process_with_a_typed_terminal_result` — passed.
- Targeted `cargo test -p eggwork-server lease_expiry_takes_precedence_over_cancel_before_spawn` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed (40 tests across 4 suites; RTK summary, 90.75s).

This is a narrow result-classification correction. It does not change lease ownership, cancellation, retry, or recovery policy. The original closure's stated untested renewal-at-expiry stress race remains an unqualified verification gap; this correction does not claim to test that race.

Disposition: **corrective closed**. The original M002 closure remains the historical record, and this addendum is the current authority for pre-spawn lease-expiry classification.
