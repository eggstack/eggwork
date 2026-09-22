# Execution ownership

Only `eggwork-runner` may own production process creation. The server performs immediate local admission and retains its permit until runner cleanup completes. It does not maintain a queue or select another node. The caller retains global scheduling, placement, and retry authority.

These boundaries follow [ADR-0005](../plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md) and the [foundation roadmap](../plans/subsystems/foundation-execution-core-roadmap.md). M001 defines the contracts only; process execution and network listeners are deferred to later milestones.
