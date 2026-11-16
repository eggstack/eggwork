# Eggwork Planning Registry

This is the control surface for current work. Long-term direction lives in the
[canonical plans](000-long-term-specification.md), durable decisions in
[ADRs](adrs/README.md), and implementation evidence in `closure/`. Planning
rules are in [003-planning-process.md](003-planning-process.md).

## Current status

No implementation handoff is ready. Four conditionally closed plans retain
named evidence requirements; see their closure records before reopening work.
Closed and superseded handoffs are in `archive/`. Their closure evidence stays
in `closure/`.

| Workstream | Status | Closure evidence |
|---|---|---|
| Foundation / execution core | M001–M003 and M005 closed; M004 conditionally closed | [M004](closure/foundation-execution-core/004-status.md), [M005](closure/foundation-execution-core/005-status.md) |
| Control-plane protocol | M001–M004 closed | [M004](closure/control-plane-protocol/004-status.md) |
| Workspace / artifacts | M001–M004 and M005b closed; M005a conditionally closed | [M005a](closure/workspace-artifact-transport/005a-status.md), [M005b](closure/workspace-artifact-transport/005b-status.md) |
| Security / isolation / resources | M001–M004, remote-admission C001, and M005a–M005c closed | [M005a](closure/security-isolation-resource/005a-status.md), [remote-admission C001](closure/security-isolation-resource-remote-admission-corrective/001-status.md) |
| Operations / distribution | M001–M003, M002a, M004a/b, and M008a/b closed; M004 and M007 conditionally closed | [M004](closure/operations-distribution/004-status.md), [M007](closure/operations-distribution/007-status.md), [M008a](closure/operations-distribution/008a-status.md), [M008b](closure/operations-distribution/008b-status.md) |
| Optional Gregg telemetry | M001–M002 closed | [M001](closure/optional-gregg-host-telemetry/001-status.md), [M002](closure/optional-gregg-host-telemetry/002-status.md) |
| CodeGG integration | Eggwork reference contract closed; downstream M001–M003 closed | [Eggwork closure](closure/codegg-integration/001-status.md) |

## Remaining evidence

The four conditionally closed plans are retained in `implementation/` because they
still define named evidence requirements:

- [Foundation M004 — Windows process-tree backend](implementation/foundation-execution-core/004-windows-job-object-process-tree-backend.md)
- [Workspace M005a — declared-artifact capture](implementation/workspace-artifact-transport/005a-declared-artifact-capture-fail-closed-corrective.md)
- [Operations M004 — release qualification](implementation/operations-distribution/004-operational-and-release-qualification.md)
- [Operations M007 — Windows service host](implementation/operations-distribution/007-windows-service-host.md)

The release receipt for M004 also gates the installed-service evidence for M007.
The closure records identify exactly what remains; no qualification is implied
by these plans' conditional status.

## Deferred work

- CodeGG M004 remains downstream-owned and waits on its stable AgentRun
  worker-entry contract.
- Reverse-connect and PTY support remain deferred.

Recheck external interfaces when dependency work resumes. Do not copy upstream
transport, deployment, or release functionality into Eggwork.
