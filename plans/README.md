# Eggwork Planning System

This directory separates durable product and architecture direction from temporary execution planning.

## Canonical long-term documents

The following files define the intended Eggwork product and architecture and MUST NOT be edited as part of ordinary implementation work:

- `000-long-term-specification.md` — normative end-state specification and invariants.
- `001-terminology-and-domain-model.md` — normative language, identities, and ownership model.
- `002-long-term-roadmap.md` — dependency-ordered capability roadmap.
- `003-planning-process.md` — rules for deriving and managing interim plans.

The first three documents are stable architectural references. Changes require an explicit long-term architecture decision, not implementation convenience. Interim plans MUST reference them rather than silently revising their requirements.

## Planning hierarchy

```text
Long-term specification and terminology
        |
        v
Architecture decision records
        |
        v
Master long-term roadmap
        |
        v
Subsystem roadmaps
        |
        v
Milestone implementation plans
        |
        v
Implementation and verification
        |
        v
Closure records and archive
```

## Directory roles

- `adrs/` — durable architecture decisions. Accepted decisions are superseded, not rewritten.
- `subsystems/` — subsystem specifications and dependency-ordered roadmaps.
- `implementation/` — bounded milestone plans handed to implementation agents.
- `closure/` — verification, evidence, residual-risk, and completion records.
- `archive/` — completed or superseded interim planning retained for traceability.
- `registry.md` — compact control surface for active work, dependencies, and handoff state.

## Core rule

Long-term documents state **what Eggwork is becoming and what must remain true**. Interim documents state **what an agent should implement next against a specific repository baseline**.

Implementation agents MUST NOT add commit-specific steps, transient file lists, current test counts, or short-lived corrective work to canonical long-term documents.

## Required architectural boundary

Eggwork is a generic fixed-target remote execution fabric. It executes a bounded request on a caller-selected node and returns machine-readable events, artifacts, and terminal state.

Eggwork MUST NOT become a scheduler. In particular it does not own global queues, DAGs, priority/fairness policy, automatic worker selection, cross-node placement, semantic retry policy, CodeGG AgentRun semantics, or workflow orchestration.

A node MAY enforce protective local admission such as maximum concurrent executions, disk/output limits, drain state, and required isolation. Saturation returns a typed refusal such as `busy`; the caller decides whether to wait, retry, select another node, or fail.

## Planning lifecycle

1. Identify the relevant long-term requirements and invariants.
2. Record unresolved durable architecture decisions in `adrs/`.
3. Create or update the relevant subsystem roadmap.
4. Select one dependency-ready milestone.
5. Write a bounded handoff plan under `implementation/`.
6. Implement and verify the milestone.
7. Write a closure record under `closure/`.
8. Update `registry.md` and the subsystem roadmap.
9. Archive superseded interim documents without rewriting accepted evidence.

No milestone is complete merely because code landed. Completion requires the closure evidence defined by its implementation plan and subsystem roadmap.

## Required classification

Every subsystem roadmap and implementation plan MUST distinguish:

- **Invariant** — a property that must always remain true.
- **Capability** — user-, operator-, or integration-visible behavior.
- **Infrastructure** — internal machinery required by capabilities.
- **Polish** — ergonomics, diagnostics, performance tuning, cleanup, or documentation.

Infrastructure and polish MUST NOT be represented as completed user capability unless the observable acceptance criteria are satisfied.

## Naming conventions

- ADR: `adrs/ADR-NNNN-short-title.md`
- Subsystem roadmap: `subsystems/<subsystem>-roadmap.md`
- Milestone implementation plan: `implementation/<subsystem>/NNN-short-title.md`
- Closure record: `closure/<subsystem>/NNN-status.md`
- Archived document: retain its original relative structure beneath `archive/`

Use stable subsystem names. Do not encode dates in filenames unless the document is inherently time-bound.

## Founding architecture

Initial durable decisions are recorded in:

- `adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `adrs/ADR-0003-execution-idempotency-leases-and-fencing.md`
- `adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`
- `adrs/ADR-0005-runner-service-separation-and-local-admission.md`

Initial subsystem decomposition:

- foundation and execution core;
- control-plane protocol;
- workspace and artifact transport;
- security, isolation, and resource enforcement;
- operations and distribution;
- CodeGG integration.

Register active work in `registry.md` before handing implementation plans to agents.
