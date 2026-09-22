# Execution ownership

Only `eggwork-runner` may own production process creation. `LocalProcessRunner` accepts argv, executes inside a caller-authorized root, clears and rebuilds the environment from a conservative baseline, writes bounded stdin, and concurrently drains stdout and stderr. Capture retains bounded head/tail bytes; a full event channel drops stream chunks while pipe draining continues. Timeout, cancellation, terminate-on-overflow, and leader exit with remaining descendants signal the process group and force-kill after a grace period. Cleanup diagnostics do not rewrite a known exit outcome.

On non-Unix hosts the runner returns an explicit unsupported-platform error until a process-tree backend is implemented and host-qualified. Linux was exercised for this milestone. The server performs immediate local admission in a later plan and retains its permit until runner cleanup completes. It does not maintain a queue or select another node. The caller retains global scheduling, placement, and retry authority.

These boundaries follow [ADR-0005](../plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md) and the [foundation roadmap](../plans/subsystems/foundation-execution-core-roadmap.md). Network listeners remain deferred to later milestones.
