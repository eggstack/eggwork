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

- `c33e9d6` — Operations M002a recovery-safe Eggup lifecycle composition;
- `b760c1a` — Operations M004 deployment/service-policy implementation;
- `68a6cc2` — runner output-monitor completion-race correction found by exact
  staged-release mTLS smoke;
- `1fcd426` — latest recorded M004 qualification/CI evidence before the
  current planning rebaseline;
- `fc67f8d` — Operations M004 release-determinism and qualification harness,
  and the source of the corrected `v0.1.1` candidate;
- `caa3918` — Windows service management fails closed before any mutation;
- `202d395` — platform-shaped execution child environment;
- `ec2b0e2` — Windows platform-disposition qualification job;
- historical `v0.1.0` producer run `36787942079` and same-tag rerun
  `36868105194`; corrected `v0.1.1` runs `37090342717`, `37091624589`, and the
  operational-qualification runs recorded in the M004 closure record.

Implemented and closed/qualified at that baseline:

- Foundation M001-M003 plus CI corrective C001;
- Control Plane M001-M003 plus lease-expiry correction;
- Workspace/Artifact M001-M004;
- Security M001-M004 plus the Landlock/null-device and remote-admission
  correctives;
- Operations M001-M003 plus M002a;
- Operations M004 **conditionally closed** (see
  `plans/closure/operations-distribution/004-status.md`).

Operations M004 is conditionally closed on an Eggwork-owned deterministic
Windows policy and a corrected `v0.1.1` draft. Three evidence items remain
outstanding and are named in the closure record: public publication with a
post-publication bootstrap matrix (a human action, plan §14), Windows service
management (dispositioned `unsupported`; the daemon has no service-control
dispatcher, so registering a service host is a separate product milestone), and
Windows child execution (the candidate built a Unix-only child environment;
fixed on `main`, re-qualification needs a release containing `202d395`). The
historical `v0.1.0` release remains immutable failure/discovery evidence and
must never be published as the qualified release.
The repository is no longer planning-only.

## Active workstreams

| Workstream | Status | Current work | Authority |
|---|---|---|---|
| Foundation / execution core | closed | M001-M003 closed; C001 portable ownership-guard CI enforcement closed | `plans/subsystems/foundation-execution-core-roadmap.md`, `plans/subsystems/foundation-execution-core-post-closure-ci-corrective-addendum.md` |
| Control-plane protocol | closed | M001-M003 complete | `plans/subsystems/control-plane-protocol-roadmap.md` |
| Workspace / artifacts | closed | M001-M004 closed, including reusable manifest CAS + derived materialization (`plans/closure/workspace-artifact-transport/004-status.md`) | `plans/subsystems/workspace-artifact-transport-roadmap.md` |
| Security / isolation / resources | closed | M001-M004 plus remote-admission corrective C001 are closed (`plans/closure/security-isolation-resource/004-status.md`); macOS/Windows hosted qualification remains future Operations M004 work, not a Security defect | `plans/subsystems/security-isolation-resource-roadmap.md`, `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md` |
| Operations / distribution | conditionally closed | M001-M003 and M002a closed; M004 conditionally closed on corrected `v0.1.1` (`402292969`, tag `b60c895` -> `fc67f8d`) with systemd and launchd service-qualified, Linux required Landlock isolation qualified, full update/rollback evidence, and all-asset same-tag reuse proven. Outstanding: public publication/bootstrap, Windows service management (`unsupported`), Windows child execution (not claimed for the candidate). | `plans/subsystems/operations-distribution-roadmap.md`, `plans/closure/operations-distribution/004-status.md` |
| CodeGG integration | Eggwork reference closed; downstream M001+C001+M002+M002a+M003 closed | CodeGG required-Landlock live qualification closed at `f5f8d96d`; downstream M003 closed at CodeGG `d51afe46` (implementation `1ce377ce`, hosted `36745285774`) against this repository's Workspace M004 contract; only downstream M004 remains, deferred. | `plans/subsystems/codegg-integration-roadmap.md` |

## Dependency-ready implementation plans

These plans are independent enough to execute in parallel. Closure must reconcile any shared-file conflicts.

| Workstream | Milestone | Status | Implementation plan | Handoff note |
|---|---|---|---|---|
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
| Operations | M003 Eggpack producer packaging integration | closed | `plans/closure/operations-distribution/003-status.md` (implementation `012383b` plus hosted-run portability correctives `4b95443`/`8827ed4`; `release/eggpack/` producer configuration, generated `.github/workflows/release.yml`, CI drift guard, static release/deployment parity suite). Hosted evidence: run `36787942079`, all 20 jobs green across the five required targets; draft `eggwork v0.1.0` (release id `400502116`, 17 assets) staged, nothing published, tag unmoved by the workflow. |
| Operations corrective | M002a RecoveryRequired restart suppression + lifecycle composition | closed | `plans/closure/operations-distribution/002a-status.md` (implementation `c33e9d6`; published Eggup 0.1.1 lifecycle composition) |
| Operations | former combined M002 packaging/services/Eggup | superseded | `plans/implementation/operations-distribution/002-packaging-services-and-eggup.md`; split because Eggpack now owns producer packaging |
| CodeGG | M001 fixed-target remote executor (Eggwork reference contract) | closed | `plans/closure/codegg-integration/001-status.md`; downstream CodeGG M001+C001+M002+M002a+M003 are now closed, and downstream M004 is the only milestone left, deferred on the stable AgentRun worker-entry contract. |

## Planned / blocked work

| Work | Status | Blocker / rationale |
|---|---|---|
| Operations M004 operational/release qualification | **conditionally closed** | `plans/closure/operations-distribution/004-status.md`; deterministic MSVC policy and double-build proof landed, corrected `v0.1.1` cut and staged, all 17 assets reused on same-tag rerun without clobber, native service/update evidence collected for systemd and both launchd targets. Acceptance criterion 5 is unmet for Windows by the plan's own §15 `unsupported` disposition, not by omission. |
| Operations M005 reverse-connect relay | deferred (not unblocked) | M004 neither implements nor claims a remote connect/accept path, so it does not supply the interface M005 relies on. No immediate product need; stable identity/lease semantics already exist. |
| Operations M006 PTY extension | deferred (not unblocked) | PTY capability streams are out of M004 scope. Requires a separate interactive ownership/attach design. |
| CodeGG M003 content-aware derived workspace transfer | closed downstream | Closed in CodeGG at `d51afe46` (implementation `1ce377ce`; hosted `36745285774` success, live derived reuse under required isolation) on Eggwork `e6a5d82`; closure record `dbowm91/codegg: plans/closure/eggwork-fixed-target-remote-execution/003-status.md` |
| CodeGG M004 remote AgentRun worker | deferred | M003 is closed; only the stable AgentRun worker-entry contract remains outstanding |
| Operations M007 Windows service host | **ready** | `plans/implementation/operations-distribution/007-windows-service-host.md`. The M004 closure path for Windows service management: the daemon must call `StartServiceCtrlDispatcherW` and report `SERVICE_RUNNING` only once the node is serving, or SCM reports 1053. Interim state is correct and safe — `v0.1.2` refuses the service verbs before mutating any state — and that refusal must stay until a release containing a real host has hosted Windows evidence. No hard dependency; M005/M006 remain deferred and unrelated. |

Operations M003, M002a, and conditionally M004 are closed. M004 used the historical `v0.1.0` draft only as discovery evidence and qualified a corrected `v0.1.1` draft containing the runner fix and an Eggwork-owned deterministic Windows link policy. Do not widen M004 into runtime release discovery, service-manager duplication, or Eggpack/Eggup interoperability. Windows service hosting is a **new** milestone, not an M004 continuation, because it requires a service-control dispatcher that the daemon does not have.

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

Eggpack producer chain [CLOSED through CI M003g] --> Operations M003 [CLOSED]
Operations M002 [historically CLOSED] --> Operations M002a [CLOSED corrective]
                                              |
Operations M003 [CLOSED] ---------------------+--> Operations M004 [CONDITIONALLY CLOSED]
                                                                |
Operations Windows service host [NOT PLANNED: new milestone] <----+--> Windows service management
Operations release publication  [HUMAN ACTION: §14]       <----+--> public bootstrap evidence
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
| Eggup | published `v0.1.1` -> `881c95ff069d3d465a282cb6a495ba6fcb70cb6f`; current main reviewed `0f791324be406b9a07dd35c1c1121279bc38dcdb` | Eggwork remains pinned to registry `eggup-core`/`eggup-service` 0.1.1. That published service crate includes systemd/launchd/Windows SCM plus `commit_with_lifecycle`, whose `RecoveryRequired` contract suppresses automatic restart; newer main service work is not required for M002a/M004. |
| Eggpack | current reviewed main `56ed7e747fd39e4d6a32a9f1fe3e09dd44355069`; Eggwork remains pinned to durable `32a0903936fcc283863e0bfb86151b13b4d75ce9` | producer authority; the pinned revision contains qualified M003e/f/g behavior and M003h reconciliation, and `eggpack ci check` is zero-drift. Windows byte determinism is product-owned and does not require an Eggpack change. Runtime Eggup-manifest consumer adoption remains separate. |
| eggsact | `f1352101dab748066c788e65e21e1bf303cfe995` | prior-art evidence only: its product-owned MSVC `/BREPRO` + `/DEBUG:NONE` correction demonstrates the mechanism behind the same Windows mismatch; Eggwork must implement and qualify its own policy and does not depend on eggsact closure |
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
11. **Stale upstream baselines:** reconciled. CodeGG M003 is closed (`d51afe46`, implementation `1ce377ce`, hosted `36745285774`). Eggpack's `main` head is now `404f63e`, which records the Eggwork producer-only adoption boundary in the Eggpack registry and ecosystem/eggup-interoperability roadmaps. Operations M003 is implemented and closed; its tool pin is Eggpack `8507fbe` rather than the plan's `398cd43` candidate, because the CI M003e/f/g correctives between them fixed real generated-workflow execution defects and `8507fbe` carries the only hosted end-to-end five-target evidence. That pin lives on a published non-`main` branch and is a recorded low-severity dependency.
12. **Operations M003 hosted-evidence gap:** resolved. Runs `36784830178` and `36786345079` found two bounded Eggwork-side portability defects (unconditional `landlock` dependency; a cfg-dependent inference gap), both corrected inside M003 (`4b95443`, `8827ed4`); run `36787942079` is green across all five required targets and staged draft `eggwork v0.1.0` with 17 assets.
13. **M002 RecoveryRequired restart gap:** resolved. M002a implemented published Eggup 0.1.1 `commit_with_lifecycle` at `c33e9d6`, preserves the public compatibility API, and suppresses automatic restoration on `RecoveryRequired`; closure is `plans/closure/operations-distribution/002a-status.md`.

14. **Windows rerun ownership diagnosis:** corrected. The `v0.1.0` rerun mismatch is not an Eggpack blocker and does not require eggsact to close. Eggwork itself lacks deterministic MSVC link policy. eggsact `f1352101` demonstrates the product-side `/BREPRO` + `/DEBUG:NONE` approach; M004 now requires Eggwork-local adoption plus an independent two-build/reuse proof.
15. **M004 release candidate:** rebaselined. The immutable `v0.1.0` draft contains the runner warning found by live smoke, so it remains historical evidence. M004 success targets a new coherent `v0.1.1` candidate after the runner fix, Windows determinism guard, and version bump are present.

## Remaining interface gates

1. **Eggup publication:** satisfied for M002 — published `eggup-core 0.1.1` / `eggup-service 0.1.1` (checksums in `Cargo.lock`) supply the M007 `commit_with_post_commit` seam and manager adapters; no floating branch was used. Eggwork's producer path is already hosted-qualified by M003 run `36787942079`; M004 owns installed/service/update qualification and any authorized publication/public-bootstrap evidence.
2. **Eggpack producer contract:** satisfied and consumed. Eggwork pins durable Eggpack `32a0903936fcc283863e0bfb86151b13b4d75ce9`; generated workflow drift is zero. No producer-interface blocker remains for M004. Windows byte determinism is Eggwork product build policy, while Eggpack correctly keeps same-name digest mismatches fail-closed.
3. **Platform qualification:** Linux is the exercised execution/security platform. For release packaging, all five required targets now have hosted build/qualification/validation evidence (run `36787942079`); installed first-install smoke and service lifecycle/update/rollback on qualified hosts remain M004 work. Windows/macOS runtime claims beyond the producer pipeline still require hosted M004 evidence; cross-compilation is not qualification.
4. **Eggress feature scope:** SOCKS5 and HTTP CONNECT should be the required deterministic route fixtures. SSH is advertised only if its feature/runtime path is actually exercised.

## Planning hygiene

- Register before handoff.
- Historical closure evidence is immutable; use corrective records for later findings.
- Do not copy Eggfetch/Eggress/EggServe/Eggup/Eggpack functionality into Eggwork to avoid an upstream boundary.
- Do not treat a plan as implementation evidence.
- Keep scheduling/placement outside Eggwork.
- Parallel-ready work may proceed independently, but each closure must verify the actual merged head before unblocking downstream milestones.
