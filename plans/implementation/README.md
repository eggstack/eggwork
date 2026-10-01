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

Foundation M001-M003, Control Plane M001-M003, Workspace M001-M004, Security M001-M004 plus corrective work, and Operations M001-M003 have closure evidence. See `plans/registry.md` for the compact controlling status.

## Current Eggwork work

Operations M002-M003 and corrective M002a are closed. Operations M004 is the
active handoff and has been rebaselined after partial qualification.

Resume M004 at:

- `operations-distribution/004-operational-and-release-qualification.md`

The next implementation/qualification tranche is Eggwork-owned:

- add deterministic Windows MSVC link policy and independent two-build proof;
- bump the workspace to corrected patch release `0.1.1`;
- stage and same-tag rerun the new `v0.1.1` draft without clobber;
- qualify corrected installed runtime bytes;
- obtain native service/isolation/update evidence, or fail closed on any
  service backend that cannot be natively qualified.

The historical `v0.1.0` draft remains immutable discovery evidence and is not
the M004 success candidate. Workspace M004 and Security M004 remain closed.

The `foundation-execution-core-ci-corrective/001-portable-ownership-guard-ci-enforcement.md` handoff has closed; closure evidence lives in `plans/closure/foundation-execution-core-ci-corrective/001-status.md`.

CodeGG integration implementation authority has moved to the CodeGG repository. Eggwork retains its integration-contract roadmap as architecture/reference material, but downstream CodeGG changes should be planned, implemented, and closed in CodeGG. Downstream CodeGG M001+C001+M002+M002a+M003 are closed; only M004 remains deferred on CodeGG's stable AgentRun worker-entry contract.

## Later work

- Reverse-connect and PTY work remain deferred.
- CodeGG M004 is downstream-owned and waits only on the stable AgentRun worker-entry contract.

The superseded `operations-distribution/002-packaging-services-and-eggup.md` remains historical evidence of the pre-Eggpack ownership split.
