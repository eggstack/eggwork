# Architecture overview

Eggwork is a fixed-target execution fabric. A caller selects a node; Eggwork performs local bounded admission and execution. Queueing, placement, fairness, and retry policy remain with the caller. See the canonical specification and [ADR-0001](../plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md).

Crate ownership:

- `eggwork-core`: transport-neutral identities, requests, results, validation, and events.
- `eggwork-runner`: the one process lifecycle owner; depends on core.
- `eggwork-server`: node service and admission; depends on core and runner.
- `eggwork-client`: explicit fixed-target client; depends on core. Its optional `eggress-route` feature adds an in-process outbound route without changing the target identity.

The dependency graph is intentionally one-way. Details are in [domain](domain.md), [execution ownership](execution-ownership.md), and [protocol intent](protocol.md).
