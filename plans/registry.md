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

Latest Eggwork implementation/closure head reviewed before this corrective registration:

- `faaa0b905fa6bc43e46825fdd98530b5533a970f` — CodeGG M001 Eggwork reference-contract closure.

Implemented and closed/qualified at that baseline:

- Foundation M001-M003, plus the post-closure CI enforcement corrective C001;
- Control Plane M001-M003 plus lease-expiry correction;
- Workspace/Artifact M001-M003;
- Security M001-M003 plus Landlock `/dev/null` corrective;
- Operations M001.

The repository is no longer planning-only.

## Active workstreams

| Workstream | Status | Current work | Authority |
|---|---|---|---|
| Foundation / execution core | closed | M001-M003 closed; C001 portable ownership-guard CI enforcement closed | `plans/subsystems/foundation-execution-core-roadmap.md`, `plans/subsystems/foundation-execution-core-post-closure-ci-corrective-addendum.md` |
| Control-plane protocol | closed | M001-M003 complete | `plans/subsystems/control-plane-protocol-roadmap.md` |
| Workspace / artifacts | closed | M001-M003 complete | `plans/subsystems/workspace-artifact-transport-roadmap.md` |
| Security / isolation / resources | C001 closed; M004 blocked on Operations M002 | M002/M003 historical closures remain qualified; remote-admission corrective C001 is closed (`plans/closure/security-isolation-resource-remote-admission-corrective/001-status.md`); M004 waits on Operations M002 | `plans/subsystems/security-isolation-resource-roadmap.md`, `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md` |
| Operations / distribution | ready | M002 re-opened on qualified Eggup M007 post-commit rollback seam; producer packaging remains separate M003 | `plans/subsystems/operations-distribution-roadmap.md` |
| CodeGG integration | Eggwork reference closed; downstream M001+C001 closed | CodeGG lease/live-node corrective closed at `d3d390d5`; M002 is registered/ready and M002a waits on M002 + Eggwork remote-admission C001. | `plans/subsystems/codegg-integration-roadmap.md` |

## Dependency-ready implementation plans

These plans are independent enough to execute in parallel. Closure must reconcile any shared-file conflicts.

| Workstream | Milestone | Status | Implementation plan | Handoff note |
|---|---|---|---|---|
| Operations | M002 Eggup deployment/service integration | **ready** | `plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md` | Eggup M007 closure `2cab1f97` supplies `commit_with_post_commit` and rollback-on-health-failure semantics; historical blocker remains in `plans/closure/operations-distribution/002-status.md`. |
| CodeGG downstream | C001 lease identity + live-node qualification | **closed in CodeGG** | `dbowm91/codegg: plans/closure/eggwork-fixed-target-remote-execution-corrective/001-status.md` | Closure/current head `d3d390d5`; real mTLS qualification passed and M002 is eligible for planning. The live fixture exposed this repo's remote-admission C001 gap. |

## Closed and superseded implementation plans

| Workstream | Milestone | Status | Evidence / note |
|---|---|---|---|
| Foundation | M001 domain/bootstrap | closed | `plans/closure/foundation-execution-core/001-status.md` |
| Foundation | M002 local runner | closed | `plans/closure/foundation-execution-core/002-status.md` |
| Foundation | M003 execution-ownership guards + runner API hardening | closed | `plans/closure/foundation-execution-core/003-status.md` |
| Foundation corrective | C001 portable ownership guard + CI enforcement | closed | `plans/closure/foundation-execution-core-ci-corrective/001-status.md` |
| Control plane | M001 authenticated execution | closed | `plans/closure/control-plane-protocol/001-status.md` |
| Control plane | M002 idempotency/leases/events/recovery | closed | `plans/closure/control-plane-protocol/002-status.md` + lease-expiry correction |
| Workspace | M001 blob store | closed | `plans/closure/workspace-artifact-transport/001-status.md` |
| Workspace | M002 materialization | closed | `plans/closure/workspace-artifact-transport/002-status.md` |
| Workspace | M003 artifacts/retention/GC | closed | `plans/closure/workspace-artifact-transport/003-status.md` |
| Security | M001 authz/redaction/threat model | closed | `plans/closure/security-isolation-resource/001-status.md` |
| Security | M002 Landlock | closed | `plans/closure/security-isolation-resource/002-status.md` |
| Security | M002a `/dev/null` corrective | closed | `plans/closure/security-isolation-resource/002a-status.md` |
| Security | M003 resource enforcement | qualified/closed | `plans/closure/security-isolation-resource/003-status.md` |
| Security corrective | C001 remote enforcement admission + capability truthfulness | closed | `plans/closure/security-isolation-resource-remote-admission-corrective/001-status.md` |
| Operations | M001 node operations | closed | `plans/closure/operations-distribution/001-status.md` |
| Operations | former combined M002 packaging/services/Eggup | superseded | `plans/implementation/operations-distribution/002-packaging-services-and-eggup.md`; split because Eggpack now owns producer packaging |
| CodeGG | M001 fixed-target remote executor (Eggwork reference contract) | closed | `plans/closure/codegg-integration/001-status.md` (reviewed head `4efc91efa53da06663c9bf7d6a948fa240122454`; zero production changes; Phases 0-5 contract re-verified; downstream CodeGG C001 still gates current qualification) |

## Planned / blocked work

| Work | Status | Blocker / rationale |
|---|---|---|
| Security M004 adversarial + cross-platform closure | blocked | wait for Operations M002 so final evidence covers remotely reachable Landlock/resource controls and deployment/update surfaces (Security remote-admission C001 is closed) |
| Operations M003 Eggpack producer packaging | blocked | Eggpack ReleaseManifest M001/M001a is closed; Build/Qualification M001 and downstream release-orchestration interfaces are not yet closed/concrete |
| Operations M004 operational/release qualification | blocked | Operations M002 + M003 |
| Operations M005 reverse-connect relay | deferred | no immediate product need; stable identity/lease semantics already exist |
| Operations M006 PTY extension | deferred | requires a separate interactive ownership/attach design |
| CodeGG M002 capability/target projection | ready downstream | CodeGG plan `002-target-capability-projection-and-operator-policy.md` is registered/ready and may proceed against this repo's written capability contract |
| CodeGG M002a restricted-spec live requalification | blocked downstream | requires CodeGG M002 closure (Eggwork Security remote-admission C001 is closed); this is the gate for claiming real required-Landlock execution through CodeGG |
| CodeGG M003 Git-aware materializer | deferred | optimized Eggwork materializer contract; downstream CodeGG corrective C001 is closed |
| CodeGG M004 remote AgentRun worker | deferred | CodeGG M002-M003 + stable worker-entry contract |

Do not author Operations M003 merely to increase plan count. Its producer inputs are still moving in Eggpack.

## Corrected execution graph

```text
Foundation M003 [closed] --+--> Foundation corrective C001 [CLOSED]
Control Plane M003 [closed]
Operations M001 [closed] + Eggup M007 ------> Operations M002 [READY]

Eggwork Phases 0-5 [historical] -----------> CodeGG M001 [IMPLEMENTED/HISTORICALLY CLOSED]
                                                   |
                                                   +--> CodeGG C001 lease/live-node corrective [CLOSED]
                                                              |
                                                               +--> exposed Eggwork remote-admission C001 [CLOSED]

Security remote-admission C001 [CLOSED] --+
Control M003 [closed] -----------------+--> Security M004 [blocked until Operations M002 closes]
Operations M002 -----------------------+

Eggpack build/qualification + release interfaces -> Operations M003 [blocked]
Operations M002 + Operations M003 ----------> Operations M004
```

Operations M002 is not a dependency of CodeGG M001. The canonical long-term roadmap already defines CodeGG Phase 7 as depending on Eggwork Phases 0-5, not completion of Phase 6 packaging.

## Current upstream interface baselines

These are reviewed research baselines, not permanent dependency pins.

| Project | Reviewed baseline | Current relevant state |
|---|---|---|
| CodeGG | `d3d390d56620fb5c6f755a5dfb0987e0a1c01651` | M001 + post-closure C001 qualified with real mTLS/live-node evidence; M002 eligible for planning; live restricted-spec test exposed Eggwork's server admission gap |
| Eggfetch | `b90b32541bd5dac6c5256feed141cadfd124debe` / `eggfetch-core 0.2.0` | public advanced-routing `Dialer` and `ClientBuilder::dialer` preserve Eggfetch-owned HTTP/TLS |
| Eggress | `e141d4082d211cc5f122414c74617fde846ffbae` / 1.0.8 line | listener-free `OutboundConnector::connect_tcp_detailed` returns a Tokio stream and typed route failure facts |
| EggServe | `dc8ce30e2c9f0a95f73682cea07163e8076995bd` / 0.2.x line | server/TLS service baseline; re-check exact published patch before dependency changes |
| Eggup | `2cab1f97ef30fa347c2030da321462459672c521` | M007 closed/qualified; `commit_with_post_commit` retains backups through caller health validation and supports `KeepInstalled | RollBack`; Unix manager + Windows SCM work also available |
| Eggpack | `0b1c3b9795280dea610c2fbe9d1533591d610b3f` | producer authority; ReleaseManifest M001/M001a closed; Build/Qualification M001 is ready for its own handoff but not yet a closed integration interface |
| Gregg | `5b2c7e8c67616f872070e2acdcce90790f750fbf` | telemetry/protocol patterns only; not an execution control plane |

Implementation agents MUST re-check current APIs at execution time.

## Resolved planning defects

1. **False Operations → CodeGG dependency:** removed. CodeGG M001 is independently ready.
2. **Foundation M003 stale blocker:** M002 is closed; M003 now has a concrete ready handoff.
3. **Control Plane M003 stale dial-interface blocker:** Eggfetch/Eggress now expose a compatible public raw-stream boundary; M003 is ready.
4. **Operations M002 blocker:** the historical post-commit rollback gap was real at Eggup `66813b3b...`; Eggup M007 later closed that exact seam, so M002 is re-opened. Historical blocker evidence remains immutable.
5. **Eggup/Eggpack authority mix:** corrected. Eggup owns local deployment/service lifecycle; Eggpack owns producer packaging/release construction/evidence.
6. **Final security sweep ordering:** M004 now waits for the newly ready route/ownership/deployment surfaces instead of incorrectly claiming only M001-M003 prerequisites.
7. **CodeGG execution authority:** the downstream implementation handoff is now registered in the CodeGG repository; Eggwork's M001 document is reference-contract material only.
8. **Foundation M003 CI enforcement defect:** registered as corrective C001 because the guard used undeclared `rtk` and was not invoked by ordinary GitHub Actions; C001 is now closed (`plans/closure/foundation-execution-core-ci-corrective/001-status.md`) — the guard calls `cargo metadata` directly, runs as a required GitHub Actions step on push/PR, and a deterministic `--prove-negative-exit` mode exercises the failure path on every CI run.
9. **CodeGG M001 post-closure correctness:** CodeGG C001 is now closed at `d3d390d5`; lease identity and real-node mTLS/restart/cancel/renew behavior are qualified.
10. **Remote enforcement admission gap:** that live qualification proved Eggwork's server rejects all remote filesystem-isolation requests even though the canonical runner can enforce the closed Landlock `workspace_rw` profile. Registered as Security remote-admission corrective C001 (now closed: `plans/closure/security-isolation-resource-remote-admission-corrective/001-status.md`); network-disabled/allow-list remain intentionally unsupported.

## Remaining interface gates

1. **Eggup publication:** M002 may qualify against exact immutable Eggup M007 revision `2cab1f97...`/implementation `8d5fc12f...` if crates.io publication still lags; never use a floating branch. Release publication must record its dependency policy explicitly.
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
