# Eggwork Architecture Decision Records

ADRs record durable decisions that constrain later implementation.

## Rules

- Accepted ADRs are not rewritten to make history cleaner.
- A changed decision is recorded in a new ADR that explicitly supersedes the old one.
- ADRs describe architecture and ownership, not temporary implementation steps.
- Implementation plans MUST cite the ADRs they rely on.
- A plan that discovers a conflict with an accepted ADR must stop and request an architecture decision rather than silently diverging.

## Status values

- proposed
- accepted
- superseded
- rejected

## Founding ADRs

- `ADR-0001-fixed-target-scheduler-free-execution.md`
- `ADR-0002-eggstack-transport-and-mtls.md`
- `ADR-0003-execution-idempotency-leases-and-fencing.md`
- `ADR-0004-content-addressed-workspaces-and-artifacts.md`
- `ADR-0005-runner-service-separation-and-local-admission.md`
