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

Foundation:

1. `foundation-execution-core/001-repository-bootstrap-and-domain-contract.md`
2. `foundation-execution-core/002-canonical-local-runner.md`

Control plane:

3. `control-plane-protocol/001-authenticated-fixed-target-execution.md`
4. `control-plane-protocol/002-idempotency-leases-events-and-recovery.md`

Workspace/artifacts:

5. `workspace-artifact-transport/001-blob-store-and-digest-protocol.md`
6. `workspace-artifact-transport/002-workspace-manifest-and-safe-materialization.md`
7. `workspace-artifact-transport/003-declared-artifacts-retention-and-gc.md`

Security/isolation:

8. `security-isolation-resource/001-authorization-redaction-and-threat-model.md`
9. `security-isolation-resource/002-trusted-landlock-sandbox-path.md`
10. `security-isolation-resource/003-enforced-resource-controls.md`

Operations:

11. `operations-distribution/001-node-operations-surface.md`
12. `operations-distribution/002-packaging-services-and-eggup.md`

Downstream integration:

13. `codegg-integration/001-codegg-fixed-target-remote-executor.md`

Only the earliest dependency-ready milestone should normally be active at one time unless the registry explicitly allows parallel work. Some later plans are intentionally written while blocked so architecture and handoff intent are preserved before implementation reaches them.
