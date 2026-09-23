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
10. `security-isolation-resource/002a-landlock-runtime-devnull.md`
11. `security-isolation-resource/003-enforced-resource-controls.md`

Operations:

12. `operations-distribution/001-node-operations-surface.md`

## Current parallel-ready wave

The registry explicitly authorizes these independent handoffs to proceed in parallel:

- `foundation-execution-core/003-execution-ownership-guards-and-runner-api-hardening.md`
- `control-plane-protocol/003-eggress-route-adapter-and-protocol-hardening.md`
- `operations-distribution/002-eggup-deployment-and-service-integration.md`
- `codegg-integration/001-codegg-fixed-target-remote-executor.md` — downstream/external handoff; actual CodeGG changes remain governed by CodeGG's own planning system.

The former `operations-distribution/002-packaging-services-and-eggup.md` is superseded because it mixed Eggup consumer authority with Eggpack producer authority.

## Next blocked work

- Security M004 final adversarial/cross-platform qualification waits on Foundation M003, Control Plane M003, and Operations M002 so it tests the current surfaces.
- Operations M003 producer packaging waits on Eggpack's concrete ReleaseManifest + build/qualification interfaces.
- CodeGG M002 waits on CodeGG integration M001.
- Reverse-connect and PTY work remain deferred.

Parallel execution is allowed only where `plans/registry.md` says so. Shared-file conflicts between parallel agents must be resolved before closure evidence is accepted.
