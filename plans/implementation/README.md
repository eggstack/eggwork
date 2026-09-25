# Eggwork Implementation Plans

Implementation plans are bounded handoff documents derived from canonical direction, ADRs, and subsystem roadmaps.

## Rules

- Register a plan in `plans/registry.md` before handoff.
- Mark it `ready` only when every hard dependency is closed with evidence.
- Keep architecture decisions out of implementation improvisation.
- Preserve the scheduler-free fixed-target boundary.
- Prefer upstream Eggstack interfaces to copied implementations.
- Stop and record a blocker if a required upstream interface is absent.
- Do not silently weaken required authentication, isolation, digest validation, fencing, or bounds to make a milestone pass.

## Closed implementation sequence

Foundation M001-M003, Control Plane M001-M003, Workspace M001-M003, Security M001-M003 plus M002a, and Operations M001 have closure evidence. See `plans/registry.md` for the compact controlling status.

## Current dependency-ready Eggwork work

The registry authorizes these Eggwork-local handoffs independently:

- `operations-distribution/002-eggup-deployment-and-service-integration.md` — re-opened after Eggup M007 supplied the required post-commit rollback seam.
- `workspace-artifact-transport/004-reusable-manifest-cas-and-derived-materialization.md` — principal-scoped retained manifests + `workspace.derive.v1`; upstream prerequisite for CodeGG M003.

The `foundation-execution-core-ci-corrective/001-portable-ownership-guard-ci-enforcement.md` handoff has closed; closure evidence lives in `plans/closure/foundation-execution-core-ci-corrective/001-status.md`.

CodeGG integration implementation authority has moved to the CodeGG repository. Eggwork retains its integration-contract roadmap as architecture/reference material, but downstream CodeGG changes should be planned, implemented, and closed in CodeGG. The Eggwork M001 reference-contract milestone is closed (`plans/closure/codegg-integration/001-status.md`); downstream CodeGG M001+C001+M002+M002a are closed; CodeGG M003 is now blocked specifically on Eggwork Workspace M004.

## Blocked later work

- Security M004 final adversarial/cross-platform qualification is registered at `security-isolation-resource/004-security-closure-and-cross-platform-adversarial-qualification.md` and waits only on Operations M002.
- Operations M003 producer packaging waits on Eggpack's concrete build/qualification interfaces.
- Operations M004 waits on M002 + M003.
- Reverse-connect and PTY work remain deferred.

The superseded `operations-distribution/002-packaging-services-and-eggup.md` remains historical evidence of the pre-Eggpack ownership split.


## Registered blocked handoffs

- `security-isolation-resource/004-security-closure-and-cross-platform-adversarial-qualification.md` — registered but blocked on Operations M002 closure.
