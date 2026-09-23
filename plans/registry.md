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
- **qualified** — the implemented baseline is closed for its explicitly stated platform/scope while later expansion/closure work may remain.
- **conditionally closed** — implementation is substantially complete but named evidence remains.
- **corrective required** — a later finding must close before the area is treated as currently qualified.
- **superseded** — replaced by another document.
- **archived** — retained for traceability and no longer active.

## Current Eggwork implementation baseline

Latest implementation/closure head reviewed before this planning correction:

- `86f80d6c0ce5818b11ed272847656a5813f726c2` — Operations M001 closure.

Implemented and closed/qualified at that baseline:

- Foundation M001-M002;
- Control Plane M001-M002 plus lease-expiry correction;
- Workspace/Artifact M001-M003;
- Security M001-M003 plus Landlock `/dev/null` corrective;
- Operations M001.

The repository is no longer planning-only.

## Active workstreams

| Workstream | Status | Current work | Authority |
|---|---|---|---|
| Foundation / execution core | closed | M001-M003 complete | `plans/subsystems/foundation-execution-core-roadmap.md` |
| Control-plane protocol | closed | M001-M003 complete | `plans/subsystems/control-plane-protocol-roadmap.md` |
| Workspace / artifacts | closed | M001-M003 complete | `plans/subsystems/workspace-artifact-transport-roadmap.md` |
| Security / isolation / resources | qualified | M004 waits on the new Foundation/Control/Operations surfaces | `plans/subsystems/security-isolation-resource-roadmap.md` |
| Operations / distribution | ready | M002 Eggup deployment/service integration; producer packaging is separate M003 | `plans/subsystems/operations-distribution-roadmap.md` |
| CodeGG integration | ready external | M001 fixed-target remote executor; independent of Operations M002 | `plans/subsystems/codegg-integration-roadmap.md` |

## Dependency-ready implementation plans

These plans are independent enough to execute in parallel. Closure must reconcile any shared-file conflicts.

| Workstream | Milestone | Status | Implementation plan | Handoff note |
|---|---|---|---|---|
| Foundation | M003 ownership guards/API hardening | **closed** | `plans/implementation/foundation-execution-core/003-execution-ownership-guards-and-runner-api-hardening.md` | `plans/closure/foundation-execution-core/003-status.md` |
| Control plane | M003 Eggress route/protocol hardening | **closed** | `plans/implementation/control-plane-protocol/003-eggress-route-adapter-and-protocol-hardening.md` | `plans/closure/control-plane-protocol/003-status.md` |
| Operations | M002 Eggup deployment/service integration | **ready** | `plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md` | Eggup current main has Unix manager mechanics and native Windows SCM; exact published-or-pinned dependency disposition is part of implementation. |
| CodeGG | M001 fixed-target remote executor | **ready external** | `plans/implementation/codegg-integration/001-codegg-fixed-target-remote-executor.md` | Eggwork Phases 0-5 prerequisites are closed. Actual downstream code remains governed by CodeGG planning. |

## Closed and superseded implementation plans

| Workstream | Milestone | Status | Evidence / note |
|---|---|---|---|
| Foundation | M001 domain/bootstrap | closed | `plans/closure/foundation-execution-core/001-status.md` |
| Foundation | M002 local runner | closed | `plans/closure/foundation-execution-core/002-status.md` |
| Control plane | M001 authenticated execution | closed | `plans/closure/control-plane-protocol/001-status.md` |
| Control plane | M002 idempotency/leases/events/recovery | closed | `plans/closure/control-plane-protocol/002-status.md` + lease-expiry correction |
| Workspace | M001 blob store | closed | `plans/closure/workspace-artifact-transport/001-status.md` |
| Workspace | M002 materialization | closed | `plans/closure/workspace-artifact-transport/002-status.md` |
| Workspace | M003 artifacts/retention/GC | closed | `plans/closure/workspace-artifact-transport/003-status.md` |
| Security | M001 authz/redaction/threat model | closed | `plans/closure/security-isolation-resource/001-status.md` |
| Security | M002 Landlock | closed | `plans/closure/security-isolation-resource/002-status.md` |
| Security | M002a `/dev/null` corrective | closed | `plans/closure/security-isolation-resource/002a-status.md` |
| Security | M003 resource enforcement | qualified/closed | `plans/closure/security-isolation-resource/003-status.md` |
| Operations | M001 node operations | closed | `plans/closure/operations-distribution/001-status.md` |
| Operations | former combined M002 packaging/services/Eggup | superseded | `plans/implementation/operations-distribution/002-packaging-services-and-eggup.md`; split because Eggpack now owns producer packaging |

## Planned / blocked work

| Work | Status | Blocker / rationale |
|---|---|---|
| Security M004 adversarial + cross-platform closure | blocked | wait for Foundation M003 + Control Plane M003 + Operations M002 so final security evidence includes those surfaces |
| Operations M003 Eggpack producer packaging | blocked | Eggpack ReleaseManifest M001 and build/qualification interfaces are not yet closed/concrete |
| Operations M004 operational/release qualification | blocked | Operations M002 + M003 |
| Operations M005 reverse-connect relay | deferred | no immediate product need; stable identity/lease semantics already exist |
| Operations M006 PTY extension | deferred | requires a separate interactive ownership/attach design |
| CodeGG M002 capability/target projection | blocked | CodeGG M001 |
| CodeGG M003 Git-aware materializer | deferred | first remote executor + optimized Eggwork materializer contract |
| CodeGG M004 remote AgentRun worker | deferred | CodeGG M001-M003 + stable worker-entry contract |

Do not author Operations M003 merely to increase plan count. Its producer inputs are still moving in Eggpack.

## Corrected execution graph

```text
Foundation M002 [closed] ------------------> Foundation M003 [READY]
Control Plane M002 [closed] ---------------> Control Plane M003 [READY]
Operations M001 [closed] + Eggup adapters --> Operations M002 [READY]

Eggwork Phases 0-5 [closed/qualified] ------> CodeGG M001 [READY EXTERNAL]

Foundation M003 --+
Control M003 -----+--> Security M004 [blocked until all three close]
Operations M002 --+

Eggpack ReleaseManifest/build interfaces ---> Operations M003 [blocked]
Operations M002 + Operations M003 ----------> Operations M004
```

Operations M002 is not a dependency of CodeGG M001. The canonical long-term roadmap already defines CodeGG Phase 7 as depending on Eggwork Phases 0-5, not completion of Phase 6 packaging.

## Current upstream interface baselines

These are reviewed research baselines, not permanent dependency pins.

| Project | Reviewed baseline | Current relevant state |
|---|---|---|
| CodeGG | `7ee0a5c6abcf59370877cfb4368bc5d28eed28f9` | typed `JobExecutor`/`JobExecutionContext` and scheduler ownership remain available |
| Eggfetch | `b90b32541bd5dac6c5256feed141cadfd124debe` / `eggfetch-core 0.2.0` | public advanced-routing `Dialer` and `ClientBuilder::dialer` preserve Eggfetch-owned HTTP/TLS |
| Eggress | `e141d4082d211cc5f122414c74617fde846ffbae` / 1.0.8 line | listener-free `OutboundConnector::connect_tcp_detailed` returns a Tokio stream and typed route failure facts |
| EggServe | `dc8ce30e2c9f0a95f73682cea07163e8076995bd` / 0.2.x line | server/TLS service baseline; re-check exact published patch before dependency changes |
| Eggup | `66acd739f792437cb9fa1701b8fa456c4ba4c403` | consumer update/service authority; Unix manager mechanics and Windows SCM work are closed on main |
| Eggpack | `c5fd88f5a44a17104f00f93c9888b568272a01ea` | producer authority; ReleaseManifest M001 is current ready work, build/qualification still blocked |
| Gregg | `5b2c7e8c67616f872070e2acdcce90790f750fbf` | telemetry/protocol patterns only; not an execution control plane |

Implementation agents MUST re-check current APIs at execution time.

## Resolved planning defects

1. **False Operations → CodeGG dependency:** removed. CodeGG M001 is independently ready.
2. **Foundation M003 stale blocker:** M002 is closed; M003 now has a concrete ready handoff.
3. **Control Plane M003 stale dial-interface blocker:** Eggfetch/Eggress now expose a compatible public raw-stream boundary; M003 is ready.
4. **Operations M002 stale Eggup blocker:** Eggup main now includes platform manager mechanics; consumer deployment integration is ready.
5. **Eggup/Eggpack authority mix:** corrected. Eggup owns local deployment/service lifecycle; Eggpack owns producer packaging/release construction/evidence.
6. **Final security sweep ordering:** M004 now waits for the newly ready route/ownership/deployment surfaces instead of incorrectly claiming only M001-M003 prerequisites.
7. **Stale CodeGG baseline:** M001 planning rebaselined to current reviewed CodeGG head.

## Remaining interface gates

1. **Eggup publication:** if current service adapters are not in a published crate at implementation time, use an exact immutable Git revision for qualification or remain blocked for release publication; never use a floating branch.
2. **Eggpack producer contract:** Operations M003 remains blocked until ReleaseManifest and build/qualification interfaces close.
3. **Platform qualification:** Linux is the currently exercised execution/security platform. Windows/macOS claims require hosted runtime evidence; cross-compilation is not qualification.
4. **Eggress feature scope:** SOCKS5 and HTTP CONNECT should be the required deterministic route fixtures. SSH is advertised only if its feature/runtime path is actually exercised.

## Planning hygiene

- Register before handoff.
- Historical closure evidence is immutable; use corrective records for later findings.
- Do not copy Eggfetch/Eggress/EggServe/Eggup/Eggpack functionality into Eggwork to avoid an upstream boundary.
- Do not treat a plan as implementation evidence.
- Keep scheduling/placement outside Eggwork.
- Parallel-ready work may proceed independently, but each closure must verify the actual merged head before unblocking downstream milestones.
