# Operations and Distribution Roadmap

Status: active roadmap; M001-M003 and M002a corrective closed, M004 blocked on release and native-platform qualification evidence

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

Status: closed historically; post-closure corrective M002a is ready

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

Current interface review confirms Eggup main contains Unix manager mechanics, a native Windows SCM adapter, and Verified Update Core M007's `commit_with_post_commit` + `PostCommitFailurePolicy::{KeepInstalled, RollBack}` seam. The prior blocker remains preserved in `plans/closure/operations-distribution/002-status.md` as historical evidence; M002 is re-opened against the qualified M007 contract.

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
- CI M003b remains conditionally closed pending the byte-identical rerun-reuse receipt, which Eggpack attributes to consumer-side Windows artifact determinism (eggsact M005a). This bounds what M004 release qualification may claim: a rerun against an already-staged Eggwork Windows release will fail closed on digest mismatch rather than reconcile.
- The Eggpack tool pin `8507fbe` is published on `refs/heads/m003g-live-qualification`, not `main`. Re-pin to the merged `main` head once upstream merges CI M003e/f/g.

### M004 — Operational and release qualification

Class: polish/invariant

Status: blocked

Implementation plan:

- `plans/implementation/operations-distribution/004-operational-and-release-qualification.md`

Objective:

Qualify the staged M003 release as installed node software: native first-install/runtime smoke, platform service lifecycle, corrected update/rollback/recovery behavior, same-tag draft rerun/reuse, publication boundary, and a truthful cross-platform support matrix.

Dependency disposition:

- Operations M003 is closed at `plans/closure/operations-distribution/003-status.md`;
- the staged `eggwork v0.1.0` draft (release id `400502116`, run `36787942079`) is the starting release evidence;
- Operations M002a closed at `plans/closure/operations-distribution/002a-status.md` and its correctness prerequisite is satisfied;
- M004's same-tag run `36868105194` failed closed because the Windows binary differs from the existing draft, consistent with eggsact M005a byte reproducibility; native macOS/Windows service and installed-runtime qualification is also outstanding (`plans/closure/operations-distribution/004-status.md`).

M004 does not reopen producer packaging. It consumes the existing release assets and keeps publication human-controlled.

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
