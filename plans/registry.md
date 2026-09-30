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

Latest Eggwork implementation/closure head reviewed for this planning batch:

- `283f3ea` — Security M004 adversarial closure qualification, including the
  null-device write hardening fix and the pinned implementation SHA.

Implemented and closed/qualified at that baseline:

- Foundation M001-M003, plus the post-closure CI enforcement corrective C001;
- Control Plane M001-M003 plus lease-expiry correction;
- Workspace/Artifact M001-M004, including derived materialization;
- Security M001-M004 plus Landlock `/dev/null` corrective and the M004
  null-device write hardening fix;
- Security remote-admission corrective C001;
- Operations M001-M002.

The repository is no longer planning-only.

## Active workstreams

| Workstream | Status | Current work | Authority |
|---|---|---|---|
| Foundation / execution core | closed | M001-M003 closed; C001 portable ownership-guard CI enforcement closed | `plans/subsystems/foundation-execution-core-roadmap.md`, `plans/subsystems/foundation-execution-core-post-closure-ci-corrective-addendum.md` |
| Control-plane protocol | closed | M001-M003 complete | `plans/subsystems/control-plane-protocol-roadmap.md` |
| Workspace / artifacts | closed | M001-M004 closed, including reusable manifest CAS + derived materialization (`plans/closure/workspace-artifact-transport/004-status.md`) | `plans/subsystems/workspace-artifact-transport-roadmap.md` |
| Security / isolation / resources | closed | M001-M004 plus remote-admission corrective C001 are closed (`plans/closure/security-isolation-resource/004-status.md`); macOS/Windows hosted qualification remains future Operations M004 work, not a Security defect | `plans/subsystems/security-isolation-resource-roadmap.md`, `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md` |
| Operations / distribution | active | M002 closed; M003 Eggpack producer packaging **ready** at `plans/implementation/operations-distribution/003-eggpack-producer-packaging-integration.md`; M004 blocked on M003 | `plans/subsystems/operations-distribution-roadmap.md` |
| CodeGG integration | Eggwork reference closed; downstream M001+C001+M002+M002a+M003 closed | CodeGG required-Landlock live qualification closed at `f5f8d96d`; downstream M003 closed at CodeGG `d51afe46` (implementation `1ce377ce`, hosted `36745285774`) against this repository's Workspace M004 contract; only downstream M004 remains, deferred. | `plans/subsystems/codegg-integration-roadmap.md` |

## Dependency-ready implementation plans

These plans are independent enough to execute in parallel. Closure must reconcile any shared-file conflicts.

| Workstream | Milestone | Status | Implementation plan | Handoff note |
|---|---|---|---|---|
| Operations | M003 Eggpack producer packaging integration | **ready** | `plans/implementation/operations-distribution/003-eggpack-producer-packaging-integration.md` | Five-target producer adoption using Eggpack `3f95af43` / qualified tool pin candidate `398cd43`; Linux daemon+helper bundle, daemon-only macOS/Windows, no runtime updater or Eggpack/Eggup interoperability claim. |
| CodeGG downstream | M001+C001+M002+M002a fixed-target remote execution/policy/live isolation | **closed in CodeGG** | `dbowm91/codegg: plans/closure/eggwork-fixed-target-remote-execution/002a-status.md` | Required-Landlock production mTLS path closed at `f5f8d96d`. Downstream M003 is also closed (see below). |

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
| Workspace | M004 reusable manifest CAS + derived materialization | closed | `plans/closure/workspace-artifact-transport/004-status.md`; consumed by closed downstream CodeGG M003 |
| Security | M001 authz/redaction/threat model | closed | `plans/closure/security-isolation-resource/001-status.md` |
| Security | M002 Landlock | closed | `plans/closure/security-isolation-resource/002-status.md` |
| Security | M002a `/dev/null` corrective | closed | `plans/closure/security-isolation-resource/002a-status.md` |
| Security | M003 resource enforcement | qualified/closed | `plans/closure/security-isolation-resource/003-status.md` |
| Security | M004 adversarial + cross-platform closure | closed | `plans/closure/security-isolation-resource/004-status.md` (implementation `283f3ea`, incl. null-device write hardening fix); platform claims limited to hosted Linux with macOS/Windows fail-closed by construction |
| Security corrective | C001 remote enforcement admission + capability truthfulness | closed | `plans/closure/security-isolation-resource-remote-admission-corrective/001-status.md` |
| Operations | M001 node operations | closed | `plans/closure/operations-distribution/001-status.md` |
| Operations | M002 Eggup deployment/service integration | closed (resumed) | `plans/closure/operations-distribution/002-resumed-status.md` (implementation `d5722d9`, published `eggup-core`/`eggup-service` 0.1.1); historical pre-M007 blocker preserved immutably at `plans/closure/operations-distribution/002-status.md` |
| Operations | former combined M002 packaging/services/Eggup | superseded | `plans/implementation/operations-distribution/002-packaging-services-and-eggup.md`; split because Eggpack now owns producer packaging |
| CodeGG | M001 fixed-target remote executor (Eggwork reference contract) | closed | `plans/closure/codegg-integration/001-status.md`; downstream CodeGG M001+C001+M002+M002a+M003 are now closed, and downstream M004 is the only milestone left, deferred on the stable AgentRun worker-entry contract. |

## Planned / blocked work

| Work | Status | Blocker / rationale |
|---|---|---|
| Operations M003 Eggpack producer packaging | **ready** | Registered at `plans/implementation/operations-distribution/003-eggpack-producer-packaging-integration.md`. Eggpack producer interfaces are closed at current reviewed head `3f95af43`; qualified producer pin candidate `398cd43`. Eggwork owns its consumer config locally and does not claim Eggpack's separate runtime interoperability M003. |
| Operations M004 operational/release qualification | blocked | Operations M003 |
| Operations M005 reverse-connect relay | deferred | no immediate product need; stable identity/lease semantics already exist |
| Operations M006 PTY extension | deferred | requires a separate interactive ownership/attach design |
| CodeGG M003 content-aware derived workspace transfer | closed downstream | Closed in CodeGG at `d51afe46` (implementation `1ce377ce`; hosted `36745285774` success, live derived reuse under required isolation) on Eggwork `e6a5d82`; closure record `dbowm91/codegg: plans/closure/eggwork-fixed-target-remote-execution/003-status.md` |
| CodeGG M004 remote AgentRun worker | deferred | M003 is closed; only the stable AgentRun worker-entry contract remains outstanding |

Operations M003 now has a registered implementation handoff. Do not widen it into runtime release discovery/update, service-manager duplication, or Eggpack/Eggup interoperability solely to consume adjacent upstream milestones.

## Corrected execution graph

```text
Foundation M003 [closed] --+--> Foundation corrective C001 [CLOSED]
Control Plane M003 [closed]
Operations M001 [closed] + Eggup 0.1.1 (published) ------> Operations M002 [CLOSED]

Eggwork Phases 0-5 [historical] -----------> CodeGG M001+C001 [CLOSED]
                                                   |
                                                   +--> Eggwork remote-admission C001 [CLOSED]
                                                              |
                                                              +--> CodeGG M002+M002a [CLOSED]

Workspace M001-M003 [closed] -------------> Workspace M004 derived materialization [CLOSED]
                                                              |
                                                              +--> CodeGG M003 [CLOSED downstream; Eggwork contract consumed]

Security remote-admission C001 [CLOSED] --+
Control M003 [closed] -----------------+--> Security M004 [CLOSED]
Operations M002 [CLOSED] --------------+

Eggpack producer chain [CLOSED through Build M006 + reusable CI] --> Operations M003 [READY]
Operations M002 + Operations M003 ----------> Operations M004
```

Operations M002 is not a dependency of CodeGG M001. The canonical long-term roadmap already defines CodeGG Phase 7 as depending on Eggwork Phases 0-5, not completion of Phase 6 packaging.

## Current upstream interface baselines

These are reviewed research baselines, not permanent dependency pins.

| Project | Reviewed baseline | Current relevant state |
|---|---|---|
| CodeGG | current reviewed `0fae5c55051c895061b26b6eb5b0d5d90cd64ebc`; remote-execution M002a closure `f5f8d96d7b7371c8583196c58d36ef7b3118ed3c` | M001+C001+M002+M002a+M003 closed; M003 implemented at `1ce377ce` on Eggwork `e6a5d82` and closed at `d51afe46`; M004 deferred. Stale current-state text reconciled at CodeGG `0fae5c55`. The advance from `841ad117` is planning plus the M003 implementation, which pins this repository at `e6a5d82`. |
| Eggfetch | `b90b32541bd5dac6c5256feed141cadfd124debe` / `eggfetch-core 0.2.0` | public advanced-routing `Dialer` and `ClientBuilder::dialer` preserve Eggfetch-owned HTTP/TLS |
| Eggress | `e141d4082d211cc5f122414c74617fde846ffbae` / 1.0.8 line | listener-free `OutboundConnector::connect_tcp_detailed` returns a Tokio stream and typed route failure facts |
| EggServe | `dc8ce30e2c9f0a95f73682cea07163e8076995bd` / 0.2.x line | server/TLS service baseline; re-check exact published patch before dependency changes |
| Eggup | `2cab1f97ef30fa347c2030da321462459672c521` | M007 closed/qualified; `commit_with_post_commit` retains backups through caller health validation and supports `KeepInstalled | RollBack`; Unix manager + Windows SCM work also available |
| Eggpack | `3f95af43f99224755c97161e0c1102a84e713e35`; qualified producer implementation `398cd43bf1597ba49bfc35b5334611aa04b16600` | producer authority; closed contract/build/qualification/finalization/bootstrap/reusable-CI interfaces satisfy Operations M003; runtime Eggup-manifest consumer adoption remains a distinct Eggpack milestone |
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
11. **Stale upstream baselines:** reconciled. CodeGG M003 is closed (`d51afe46`, implementation `1ce377ce`, hosted `36745285774`). Eggpack is re-reviewed at `3f95af43` with qualified producer implementation `398cd43`; Operations M003 is now registered/ready rather than merely ready-to-plan.

## Remaining interface gates

1. **Eggup publication:** satisfied for M002 — published `eggup-core 0.1.1` / `eggup-service 0.1.1` (checksums in `Cargo.lock`) supply the M007 `commit_with_post_commit` seam and manager adapters; no floating branch was used. Release publication of Eggwork itself must still record its dependency policy explicitly (M003/M004 scope).
2. **Eggpack producer contract:** satisfied for M003 — the ReleaseManifest, build/qualification, reusable CI composition, bootstrap, and producer-side Eggup manifest interfaces required by Eggwork are closed at Eggpack `3f95af43`. Operations M003 is registered at `plans/implementation/operations-distribution/003-eggpack-producer-packaging-integration.md`. Runtime manifest-to-update adoption remains separate and is not required for M003. Two residuals carry into M003/M004 scope rather than blocking M003: CI M003b remains conditionally closed on its live rerun evidence, and Eggpack's ordered Ecosystem Adoption sequence is independently governed. Eggwork's producer-only adoption does not consume or reorder those milestones.
3. **Platform qualification:** Linux is the currently exercised execution/security platform. Windows/macOS claims require hosted runtime evidence; cross-compilation is not qualification.
4. **Eggress feature scope:** SOCKS5 and HTTP CONNECT should be the required deterministic route fixtures. SSH is advertised only if its feature/runtime path is actually exercised.

## Planning hygiene

- Register before handoff.
- Historical closure evidence is immutable; use corrective records for later findings.
- Do not copy Eggfetch/Eggress/EggServe/Eggup/Eggpack functionality into Eggwork to avoid an upstream boundary.
- Do not treat a plan as implementation evidence.
- Keep scheduling/placement outside Eggwork.
- Parallel-ready work may proceed independently, but each closure must verify the actual merged head before unblocking downstream milestones.
