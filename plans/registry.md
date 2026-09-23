# Eggwork Active Planning Registry

This file is the compact control surface for active Eggwork planning. Detailed requirements remain in canonical documents, ADRs, subsystem roadmaps, implementation plans, closure records, corrective addenda, and Git history.

## Canonical direction

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/002-long-term-roadmap.md`
- `plans/003-planning-process.md`

Founding ADRs:

- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md`
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

## Status vocabulary

- **proposed** — plan exists but is not approved for execution.
- **ready** — hard dependencies/interfaces are satisfied; plan may be handed off.
- **active** — implementation is in progress.
- **blocked** — a named dependency/evidence requirement prevents progress.
- **closing** — implementation landed and closure evidence is being gathered.
- **closed** — closure record accepted.
- **conditionally closed** — implementation is substantially complete but named evidence remains.
- **corrective required** — a later finding must close before the area is treated as currently qualified.
- **superseded** — replaced by another document.
- **archived** — retained for traceability and no longer active.

## Repository planning baseline

Eggwork was an empty Git repository when this planning system was established on 2026-09-22.

First repository/planning commit:

- `6661ba12ac3fb8d45cb32394635b71c61d947a72` — planning-system bootstrap.

At planning bootstrap there was no Rust workspace or production code. That is historical baseline context; use the milestone and closure records below for current implementation status. No roadmap capability should be treated as implemented merely because its plan exists.

## Active workstreams

| Workstream | Status | Current dependency-ready work | Authority |
|---|---|---|---|
| Foundation / execution core | closed | M002 canonical local runner | `plans/subsystems/foundation-execution-core-roadmap.md` |
| Control-plane protocol | active | M003 Eggress route adapter waits on current public Eggfetch/Eggress dial compatibility | `plans/subsystems/control-plane-protocol-roadmap.md` |
| Workspace / artifacts | closed | M001-M003 complete; no current implementation plan | `plans/subsystems/workspace-artifact-transport-roadmap.md` |
| Security / isolation / resources | closed | M001-M003 complete for the qualified Linux user-manager path | `plans/subsystems/security-isolation-resource-roadmap.md` |
| Operations / distribution | blocked | M002 packaging/services/Eggup; requires upstream platform service-manager adapters | `plans/subsystems/operations-distribution-roadmap.md` |
| CodeGG integration | ready | M001 fixed-target remote executor | `plans/subsystems/codegg-integration-roadmap.md` |

## Dependency-ready implementation plans

| Workstream | Milestone | Status | Implementation plan | Handoff note |
|---|---|---|---|---|
| CodeGG | M001 fixed-target remote executor | **ready** | `plans/implementation/codegg-integration/001-codegg-fixed-target-remote-executor.md` | Control Plane M002, Workspace M003, and Security M003 closure evidence is available. |

## Registered implementation plan statuses

| Workstream | Milestone | Status | Implementation plan | Blocker |
|---|---|---|---|---|
| Foundation | M001 repository bootstrap + domain contract | closed | `plans/implementation/foundation-execution-core/001-repository-bootstrap-and-domain-contract.md` | Closure: `plans/closure/foundation-execution-core/001-status.md`. |
| Foundation | M002 canonical local runner | closed | `plans/implementation/foundation-execution-core/002-canonical-local-runner.md` | Closure: `plans/closure/foundation-execution-core/002-status.md`. |
| Control plane | M001 authenticated fixed-target execution | closed | `plans/implementation/control-plane-protocol/001-authenticated-fixed-target-execution.md` | Closure: `plans/closure/control-plane-protocol/001-status.md`. |
| Control plane | M002 idempotency/leases/events/recovery | closed | `plans/implementation/control-plane-protocol/002-idempotency-leases-events-and-recovery.md` | Closure: `plans/closure/control-plane-protocol/002-status.md`; current qualification: `plans/closure/control-plane-protocol/002-lease-expiry-race-correction.md`. |
| Workspace | M001 blob store/digest protocol | closed | `plans/implementation/workspace-artifact-transport/001-blob-store-and-digest-protocol.md` | Closure: `plans/closure/workspace-artifact-transport/001-status.md`. |
| Workspace | M002 safe workspace materialization | closed | `plans/implementation/workspace-artifact-transport/002-workspace-manifest-and-safe-materialization.md` | Closure: `plans/closure/workspace-artifact-transport/002-status.md`. |
| Workspace | M003 declared artifacts/retention/GC | closed | `plans/implementation/workspace-artifact-transport/003-declared-artifacts-retention-and-gc.md` | Closure: `plans/closure/workspace-artifact-transport/003-status.md`. |
| Security | M001 authorization/redaction/threat model | closed | `plans/implementation/security-isolation-resource/001-authorization-redaction-and-threat-model.md` | Closure: `plans/closure/security-isolation-resource/001-status.md`. |
| Security | M002 trusted Landlock sandbox | closed | `plans/implementation/security-isolation-resource/002-trusted-landlock-sandbox-path.md` | Closure: `plans/closure/security-isolation-resource/002-status.md`; implementation `98d13f1`. |
| Security | M002a Landlock `/dev/null` runtime correction | closed | `plans/implementation/security-isolation-resource/002a-landlock-runtime-devnull.md` | Closure: `plans/closure/security-isolation-resource/002a-status.md`; historical M002 record preserved. |
| Security | M003 enforced resource controls | closed | `plans/implementation/security-isolation-resource/003-enforced-resource-controls.md` | Closure: `plans/closure/security-isolation-resource/003-status.md`; Linux systemd user-manager path qualified. |
| Operations | M001 node operations surface | closed | `plans/implementation/operations-distribution/001-node-operations-surface.md` | Closure: `plans/closure/operations-distribution/001-status.md`. |
| Operations | M002 packaging/services/Eggup | blocked | `plans/implementation/operations-distribution/002-packaging-services-and-eggup.md` | Eggup 0.1.0 exposes manager-neutral lifecycle types and a test double, but no native platform service-manager adapters. |
| CodeGG | M001 fixed-target remote executor | ready | `plans/implementation/codegg-integration/001-codegg-fixed-target-remote-executor.md` | Control Plane M002, Workspace M003, and Security M003 closures available; held behind Operations M001/M002 in requested execution order. |

## Roadmap-level work without implementation handoff yet

These are intentionally not implementation-ready:

| Work | Status | Reason |
|---|---|---|
| Foundation M003 ownership guards/API hardening | roadmap-level | M002 made the runner surface concrete; create the focused static-guard handoff when this roadmap-level item is scheduled |
| Control Plane M003 Eggress route/protocol hardening | roadmap-level | re-check current Eggfetch custom-dial/Eggress integration after M002 |
| Security M004 adversarial closure | roadmap-level | exact corpus depends on M001-M003 implementation |
| Operations M003 operational qualification | roadmap-level | exact release/service surface not implemented |
| Operations M004 reverse-connect relay | deferred | needs stable identity/lease/event semantics |
| Operations M005 PTY extension | deferred | needs stable leases/event resume + separate PTY design |
| CodeGG M002 target policy/capability projection | roadmap-level | depends on first adapter |
| CodeGG M003 Git-aware materializer | deferred | needs neutral workspace integration first |
| CodeGG M004 remote AgentRun worker | deferred | needs finite-job integration and current CodeGG worker contract |

Do not create implementation plans for these merely to increase plan count. Write them when the prerequisite interfaces are concrete enough for a bounded handoff.

## Current execution order

1. **Control Plane M001** — authenticated fixed-target execution (closed; closure: `plans/closure/control-plane-protocol/001-status.md`).
2. **Control Plane M002** — idempotency/leases/events/recovery (closed; lease-expiry race correction recorded).
3. Control Plane M002 is closed with a recorded lease-expiry race correction; **Workspace M001 -> M002 -> M003** are now closed.
4. **Security M001-M003 are closed** for the Linux systemd user-manager path qualified in `plans/closure/security-isolation-resource/003-status.md`.
5. Operations M001 is closed. Operations M002 is the next requested milestone but remains blocked on Eggup platform service-manager adapters.
6. CodeGG M001 is dependency-ready and remains sequenced after Operations M002; do not begin it before the blocked milestone is resolved.
7. Generate later roadmap-level handoffs only after their concrete dependencies exist.

## External interface research baselines

These baselines were inspected while creating the planning set. They are not dependency pins.

| Project | Reviewed baseline | Relevant interface |
|---|---|---|
| CodeGG | `2f7d84f88070eee2a4fb9f70b6d7d5d10b01048d` | JobScheduler/JobExecutor, ManagedProcessService, WorktreeService, interactive sequence/resync, identity/audit |
| Eggfetch | `8959ca890ee34f4cf456aed648315322f1e83ef7` / 0.2.0 line | async HTTP/TLS client, streaming, mTLS, advanced routing/custom dial surface |
| Eggress | `93fafff8ec8f601b509bb347bdfa136d51998c7d` / 1.0.8 | listener-free OutboundConnector, proxy/SSH/multi-hop, typed route failures |
| EggServe | `7e52ecec0b336ef1755b3e1654ddb1438b38bcdb` / 0.2.0 line | application service, streaming/backpressure, RequestLifecycle, TLS/mTLS, eggnet-tls |
| Eggup | `892d6cc4b13a5241b179cbf5e9c7d57c16e329b2`; crates.io `eggup-core`, `eggup-acquisition`, `eggup-eggfetch`, `eggup-service` 0.1.0 reviewed on 2026-09-23 | Update transaction, acquisition contract/adapter, and service ownership contract are published; service package has no native systemd/launchd/SCM adapters |
| Gregg | `5b2c7e8c67616f872070e2acdcce90790f750fbf` / 1.0.x line | versioned capability/telemetry protocol patterns |

Implementation agents MUST re-check current published/repository interfaces at their execution baseline.

## Known upstream integration questions

These are not current blockers for Foundation M001, but must be resolved before the named milestone closes:

1. **Eggfetch <-> Eggress dial composition:** confirm the currently published Eggfetch custom Dialer/advanced-routing surface can consume the listener-free Eggress stream cleanly without a loopback listener before Control Plane M003.
2. **eggnet-tls crate consumption:** verify whether Eggwork should depend directly on the neutral crate from EggServe's workspace or through an EggServe re-export at the then-current release.
3. **Eggup service adapters:** current published crates provide stable consumer contracts but no native systemd, launchd, or Windows SCM adapter. Keep Operations M002 blocked until Eggup publishes the platform service layer; do not duplicate it in Eggwork.
4. **CodeGG extraction vs adaptation:** Foundation M002 must determine whether useful ManagedProcessService code should be copied/generalized into Eggwork first or later consumed back by CodeGG. Do not create a circular repository dependency.
5. **Windows process-tree details:** actual Job Object lifecycle must be host-qualified before Eggwork claims Windows descendant cleanup/resource enforcement.
6. **Linux cgroup delegation:** rootless/user-service capability must be probed truthfully; no assumption that cgroups v2 write access exists on every Linux node.

## Planning hygiene

- Register before handoff.
- Historical closure evidence is immutable; use corrective plans for later findings.
- A blocked external interface is a real blocker.
- Do not copy Eggfetch/Eggress/EggServe/Eggup functionality into Eggwork to avoid an upstream boundary.
- Do not treat a roadmap or implementation plan as evidence that a feature exists.
- Keep scheduler/placement policy outside Eggwork.
