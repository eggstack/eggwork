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

## Initial implementation sequence

1. `foundation-execution-core/001-repository-bootstrap-and-domain-contract.md`
2. `foundation-execution-core/002-canonical-local-runner.md`
3. `control-plane-protocol/001-authenticated-fixed-target-execution.md`
4. `control-plane-protocol/002-idempotency-leases-events-and-recovery.md`
5. `workspace-artifact-transport/001-content-addressed-workspaces-and-artifacts.md`
6. `security-isolation-resource/001-enforced-isolation-and-resource-controls.md`
7. `operations-distribution/001-node-operations-packaging-and-eggup.md`
8. `codegg-integration/001-codegg-fixed-target-remote-executor.md`

Only the earliest dependency-ready milestone should normally be active at one time unless the registry explicitly allows parallel work.
