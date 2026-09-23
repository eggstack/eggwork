# Operations M002 — Packaging, Services, and Eggup Integration

Status: superseded

Superseded by:

- `plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md`

Reason:

This plan combined two authorities that have since been separated across Eggstack:

- Eggup owns consumer-side verified installation/update/rollback and service lifecycle.
- Eggpack owns producer-side release construction, release manifests/evidence, bootstrap installers, and generated release CI.

It also recorded an obsolete blocker stating that Eggup had no native platform service-manager adapters. Current Eggup main has closed Unix manager mechanics and Windows SCM work.

Historical content is intentionally replaced by this supersession marker rather than silently reusing the stale handoff. Producer packaging will receive a separate Operations M003 plan only after Eggpack's manifest/build interfaces are concrete.
