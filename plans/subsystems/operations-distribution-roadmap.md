# Operations and Distribution Roadmap

Status: active roadmap; M001-M003 and M002a closed, M004 conditionally closed; M004a and M007 ready

Canonical authority:

- `plans/000-long-term-specification.md#21-platform-and-packaging-targets`
- `plans/000-long-term-specification.md#22-deferred-capabilities`
- `plans/002-long-term-roadmap.md#phase-6--operations-packaging-and-node-administration`
- `plans/002-long-term-roadmap.md#phase-9--reverse-connect-and-interactive-extensions`

## 1. Ownership boundary

This subsystem owns configuration, node identity/config administration UX, status/doctor/drain commands, service lifecycle, logs/metrics/operator diagnostics, packaging/release assets, Eggup integration, storage inspection/GC commands, reverse-connect transport broker, and later interactive PTY client/daemon surfaces.

It does not own process lifecycle internals, scheduler placement, or CodeGG semantics.

## 2. Durable invariants

1. Installation/update/service actions have explicit ownership.
2. Update cannot fabricate execution completion.
3. Drain blocks new acceptance without becoming a queue.
4. Operator output is secret-safe.
5. Release artifacts are checksummed and provenance is inspectable.
6. Reverse relay is transport only.
7. PTY attachment ownership is authenticated and bounded.

## 3. Milestones

### M001 — Node operations surface

Class: capability/polish

Status: closed

Closure evidence: `plans/closure/operations-distribution/001-status.md`

Implementation plan:

- `plans/implementation/operations-distribution/001-node-operations-surface.md`

Objective:

Add configuration, status, doctor, drain/undrain, execution/storage inspection, cleanup/GC commands, and stable machine-readable operator output.

### M002 — Eggup deployment and service integration

Class: infrastructure/capability

Status: closed; post-closure corrective M002a also closed

Closure evidence: `plans/closure/operations-distribution/002-resumed-status.md`
(implementation `d5722d9` against published `eggup-core`/`eggup-service`
0.1.1; historical pre-M007 blocker preserved immutably in
`plans/closure/operations-distribution/002-status.md`)

Implementation plan:

- `plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md`

Supersedes:

- `plans/implementation/operations-distribution/002-packaging-services-and-eggup.md`

Objective:

Use Eggup for consumer-side verified multi-artifact deployment, rollback, install ownership, and native service lifecycle while keeping Eggwork's drain/restart policy application-owned.

Current implementation uses published Eggup 0.1.1 for consumer deployment and service lifecycle. The prior blocker remains preserved in `plans/closure/operations-distribution/002-status.md` as historical evidence; M002a later corrected lifecycle composition without rewriting the M002 closure.

Current resumed closure target:

- `plans/closure/operations-distribution/002-resumed-status.md`

The historical `002-status.md` blocker record is immutable and MUST NOT be overwritten when this resumed implementation closes.

### M002a — RecoveryRequired restart suppression and lifecycle composition corrective

Class: corrective / recovery invariant

Status: closed

Implementation plan:

- `plans/implementation/operations-distribution/002a-recovery-required-restart-suppression-and-lifecycle-composition-corrective.md`

Finding:

M004 research found that the closed M002 `orchestrate_update` wrapper manually restarts a previously running service after any successful Core receipt, including the possible `TransactionDisposition::RecoveryRequired` state. Published `eggup-service 0.1.1` already exposes `commit_with_lifecycle`, whose contract suppresses automatic service restoration when artifact state is uncertain.

Objective:

Replace the duplicate Eggwork stop/restart composition with Eggup's published lifecycle transaction while preserving persistent drain, active-execution quiescence, the existing public compatibility surface, and exact Core recovery evidence. `RecoveryRequired` must never reach an Eggwork start/restart call.

Historical M002 closure records remain immutable. M002a closed at
`plans/closure/operations-distribution/002a-status.md` and satisfied M004's
recovery-safety prerequisite. M004 now has separate release and native-platform
qualification blockers recorded in its closure evidence.

### M003 — Eggpack producer packaging integration

Class: infrastructure/capability

Status: closed

Implementation commits: `012383b`, `4b95443`, `8827ed4`

Closure evidence: `plans/closure/operations-distribution/003-status.md`

Implementation plan:

- `plans/implementation/operations-distribution/003-eggpack-producer-packaging-integration.md`

Objective:

Map Eggwork's release target/artifact policy into Eggpack's producer-side contracts, manifests, build/qualification plan, bootstrap installer, and generated release-CI surfaces without recreating producer distribution logic in Eggwork.

Landed: `release/eggpack/` is the checked-in producer authority for the five canonical targets — a two-member daemon + Landlock-helper bundle on both Linux targets and the daemon alone on macOS and Windows — with explicit Cargo package/binary bindings, a fixed configuration-free `eggworkd version` core smoke, a bounded Python3 consumer validator for the exact Linux helper candidate, a first-install policy, a static workflow shape, and a GitHub policy pinning Eggpack at `8507fbe`. `.github/workflows/release.yml` is generated from that configuration and is drift-guarded by `eggpack ci check` in ordinary CI. No runtime Eggpack dependency was added, and `eggwork-server` still never parses producer configuration.

The registered M003 plan used closed producer APIs directly. It deliberately does not claim Eggpack's separate Eggup Interoperability M003 real-runtime-consumer milestone: Eggwork accepts local deployment candidates and owns no duplicated ReleaseManifest-to-update acquisition mapping. That cross-repo disposition was recorded in Eggpack planning at `b9baa93`/`3d1cb67`/`404f63e`.

Named outstanding evidence, carried into M004 rather than blocking it:

- None from M003's own criteria: the hosted five-target run has been obtained
  (run `36787942079`, all 20 jobs green, draft `eggwork v0.1.0` with 17
  assets staged). Only `x86_64-unknown-linux-gnu` had executed producer
  evidence before that run; all five required targets are now qualified.
  M004's first-install smoke, service lifecycle/update/rollback evidence, and
  the final macOS/Windows hosted-support disposition still belong to M004.

Residual constraints carried into M004 planning, not blockers on M003:

- Eggwork is not in Eggpack's ordered Ecosystem Adoption sequence. M003 therefore keeps all Eggwork release configuration in this repository and consumes closed Eggpack producer APIs without assuming an upstream-maintained Eggwork configuration.
- Historical same-tag rerun `36868105194` proved Eggpack's fail-closed no-clobber behavior and exposed Eggwork's own Windows MSVC nondeterminism. M004 now owns the deterministic Windows correction and a fresh corrected-candidate rerun.
- Eggpack pin durability is resolved for current use: Eggwork pins durable main revision `32a0903936fcc283863e0bfb86151b13b4d75ce9`; later Eggpack main movement does not require pin churn without a concrete producer defect.

### M004 — Operational and release qualification

Class: polish/invariant

Status: **conditionally closed** — implementation substantially complete; three
named non-critical evidence items outstanding (see *Closure disposition* below
and `plans/closure/operations-distribution/004-status.md`)

Implementation plan:

- `plans/implementation/operations-distribution/004-operational-and-release-qualification.md`

Objective:

Qualify a corrected Eggwork release as installed node software: native first-install/runtime smoke, platform service lifecycle where actually supported, corrected update/rollback/recovery behavior, byte-identical same-tag draft rerun/reuse, publication boundary, and a truthful cross-platform support matrix.

Outcome:

- Corrected candidate: annotated `v0.1.1` (`b60c895` -> source `fc67f8d`), draft
  release `402292969`, 17 assets, `draft=true`, `published_at=null`.
- Windows candidate proven byte-reproducible independently of the producer:
  two native MSVC builds, identical SHA-256
  `c419e9e8ec2d74fa7159a545539021e0ffa62713f1673064347d44e12d78acac`,
  identical COFF timestamp `2560721357`, no CodeView.
- Same-tag rerun `37091624589` reused all 17 assets (`created: false`,
  `uploaded: 0`, `reused: 17`) with unchanged asset ids, so no clobber path
  exists. The first `v0.1.1` run `37090342717` passed.
- `systemd` and `launchd` are **service-qualified** on real hosted hosts with
  full install/start/stop/start/stop/restart/uninstall evidence and adversarial
  foreign-ownership refusal on every mutating verb.
- Linux required filesystem isolation is **qualified** through the *installed*
  sandbox helper: Landlock `workspace_rw` applied, outside-workspace write
  denied. macOS required filesystem isolation refuses before spawn
  (`capability_mismatch`, HTTP 409).
- Update/rollback/recovery evidence collected on Linux and both macOS targets
  against real `v0.1.0` -> `v0.1.1` generations, including a bounded-quiescence
  refusal proven to occur *before* artifact replacement, an unreadable-store
  refusal with byte-identical restore, and a rollback that restores the prior
  generation and lifecycle state without claiming "updated".
- Windows service management is dispositioned **unsupported**: the daemon has
  no service-control dispatcher, so SCM registers the service and then fails to
  start it with Windows error 1053. Mutating verbs now fail closed before any
  mutation. Making the daemon a Windows service host is a separate product
  milestone, not an M004 continuation.
- Windows child execution is **not claimed** for the `v0.1.1` candidate: the
  runner declared a Unix-only baseline child environment. Fixed on `main` in
  `202d395`; re-qualification needs a release containing that fix.

Closure disposition:

`plans/closure/operations-distribution/004-status.md`. Acceptance criterion 5
("Windows SCM ... is live-qualified") is unmet by the plan's own §15 `unsupported`
disposition rather than by omission. Outstanding evidence:

1. public publication of draft `402292969` and a post-publication bootstrap
   matrix on all five targets — a human action under plan §14, never fabricated;
2. Windows service management qualification — requires a service-host milestone;
3. Windows child execution qualification — requires Foundation M004 to close and a release containing the qualified Windows process-tree backend.

M004 closure MUST NOT be used to claim macOS/Windows required filesystem
isolation, network isolation, Windows service management, Windows child
execution, or aarch64 Linux runtime/service behavior.

Prior disposition (superseded by the outcome above, retained for traceability):

- Operations M003 is closed at `plans/closure/operations-distribution/003-status.md`;
- Operations M002a is closed at `plans/closure/operations-distribution/002a-status.md`;
- historical draft `v0.1.0` / release id `400502116` remains immutable evidence and MUST NOT be repaired, retagged, or clobbered;
- `v0.1.0` exposed the runner output-monitor race fixed at `68a6cc2`, so it cannot be the successful M004 qualification release;
- the Windows rerun mismatch is an Eggwork product build-policy issue, not a hard eggsact dependency. eggsact `f1352101` demonstrates the same MSVC mechanism with target-scoped `/BREPRO` + `/DEBUG:NONE`, but Eggwork must adopt and independently qualify its own deterministic Windows policy;
- the revised qualification target is a new patch release candidate (normally `v0.1.1`, currently unused) whose workspace version, source tag, candidate-reported version, manifest, sidecars, and asset names all agree;
- native macOS/Windows service support requires hosted evidence. If that evidence cannot be obtained, those service mutation paths must fail closed and be documented as unqualified rather than remaining exposed on the strength of adapter/unit tests alone;
- required Linux installed-helper isolation and live update/rollback/recovery evidence remain M004 qualification work.

M004 does not reopen producer ownership or authorize publication. It reuses the M003 producer contract to stage a new corrected draft after product-side release determinism and version coherence are proven.

### M004a — Qualification helper fixture isolation corrective

Class: corrective / test-infrastructure invariant

Status: **ready**

Implementation plan:

- `plans/implementation/operations-distribution/004a-release-qualification-fixture-isolation-corrective.md`

Objective:

Close the intermittent Linux `ETXTBSY` finding recorded by the M004 closure by giving every concurrently runnable helper fixture its own immutable executable path. Tests must not replace or rewrite a helper binary another test may be executing.

This corrective changes no production behavior and should close before the final Phase-6 release qualification so flaky fixture staging cannot contaminate release evidence.

### M007 — Windows service host

Class: corrective capability

Status: **ready**

Implementation plan:

- `plans/implementation/operations-distribution/007-windows-service-host.md`

Objective:

Make `eggworkd` a real SCM service host while keeping service registration and lifecycle mutation Eggup-owned. The reviewed implementation seam uses target-scoped `windows-service 0.8.1` for the dispatcher/control/status surface rather than Eggwork-owned raw Win32 FFI.

The registered service should invoke an explicit `service-host` entry. It may report `RUNNING` only after the normal NodeServer is ready; STOP/SHUTDOWN must reuse persistent drain and execution convergence before `STOPPED`.

Dependencies: no hard dependency on Foundation M004. Both are required to remove the two Windows M004 outstanding capability gaps before final Phase-6 closure.

### M005 — Reverse-connect relay

Class: capability

Status: deferred; stable identity/lease/event semantics are available, but no immediate handoff is authorized

Objective:

Permit nodes behind NAT/firewalls to maintain an authenticated outbound link through a dumb relay while preserving the same fixed-target execution identities.

Non-goal: any worker selection, global queue, or retry routing in the relay.

### M006 — Interactive PTY extension

Class: capability

Status: deferred; requires a separate PTY ownership/attach design

Objective:

Add create/attach/detach/input/resize/terminate/resume with bounded scrollback and transport-owned attachments. This must not change noninteractive runner ownership.


## 4. Target matrix

Initial intended binary targets:

- x86_64-unknown-linux-gnu;
- aarch64-unknown-linux-gnu;
- x86_64-apple-darwin;
- aarch64-apple-darwin;
- x86_64-pc-windows-msvc.

Additional musl/ARM variants are qualification work, not assumed support.

## 5. Verification strategy

- clean-install smoke;
- exact-version update/rollback where supported;
- service manager ownership;
- running/stopped/draining update cases;
- upgrade during retained terminal records;
- restart reconciliation;
- checksum/provenance;
- cross-platform hosted CI;
- reverse relay disconnect/backpressure when implemented;
- PTY ownership/resync/adversarial input when implemented.
