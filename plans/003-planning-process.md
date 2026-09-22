# Eggwork Planning Process

Status: canonical planning-process directive

This document defines how long-term Eggwork direction becomes implementation work without allowing transient implementation details to rewrite architectural intent.

## 1. Document classes

### Canonical

- `000-long-term-specification.md`
- `001-terminology-and-domain-model.md`
- `002-long-term-roadmap.md`
- this planning-process document

Canonical documents change only through deliberate architecture review.

### ADR

An ADR records one durable decision, alternatives considered, consequences, compatibility implications, and verification expectations.

Accepted ADRs are immutable historical decisions. If the decision changes, add a superseding ADR.

### Subsystem roadmap

A subsystem roadmap translates canonical direction into milestones for one coherent ownership area. It records dependencies and exit criteria, not transient implementation trivia.

### Implementation plan

An implementation plan is an executable handoff against a current baseline. It may name expected files, APIs, tests, migrations, and stop conditions.

### Closure record

A closure record is evidence. It records what landed, exact verification, deviations, residual risk, and whether the milestone is closed, conditionally closed, blocked, or requires correction.

## 2. Status vocabulary

- **proposed** — plan exists but is not approved for execution.
- **ready** — hard dependencies are satisfied and the plan may be handed off.
- **active** — implementation is in progress.
- **blocked** — a named dependency or evidence requirement prevents execution/closure.
- **closing** — implementation landed and evidence is being gathered.
- **closed** — acceptance criteria and closure evidence are satisfied.
- **conditionally closed** — implementation is substantially complete but a named non-critical evidence item remains.
- **corrective required** — later review found a material gap in historically closed work.
- **superseded** — replaced by another plan/decision.
- **archived** — retained for traceability and no longer active.

## 3. Milestone sizing

An implementation milestone SHOULD be small enough that one agent can inspect the baseline, implement the bounded change, run focused tests, run required broader guards, update documentation, and write a truthful closure record without silently implementing the next roadmap phase.

If the work requires multiple independent architectural decisions, split it before implementation.

## 4. Dependency discipline

A blocked plan may be written early to preserve design intent.

A blocked plan MUST state its concrete blocker. It MUST NOT be handed off as ready merely because adjacent implementation exists.

A downstream plan becomes ready only after the dependency's closure evidence proves the interface it relies on.

## 5. Required implementation-plan sections

Every implementation plan SHOULD include:

1. title and status;
2. source roadmap/canonical references;
3. objective;
4. baseline/problem statement;
5. scope;
6. invariants and non-goals;
7. required production changes;
8. failure/cancellation/restart/contention semantics as relevant;
9. compatibility/migration;
10. required tests;
11. required verification commands or classes;
12. documentation updates;
13. acceptance criteria;
14. stop conditions;
15. closure evidence required;
16. handoff notes.

## 6. Required closure evidence

A closure record MUST distinguish code inspection from executed evidence.

It SHOULD include reviewed commit/head, implementation commits, a requirement-to-evidence matrix, tests/commands actually run, platform/feature matrix actually exercised, negative/adversarial tests, cancellation/restart/contention evidence where applicable, security review, compatibility review, documentation status, unresolved findings by severity, and registry/roadmap disposition.

Do not claim a platform or feature is qualified merely because its code compiles.

## 7. Corrective work

Later review may discover a gap in historically closed work.

Do not rewrite the old closure record to erase the history.

Instead:

1. add a corrective subsystem addendum or corrective plan;
2. register it as current authority;
3. describe what historical claim is no longer sufficient;
4. implement and close the corrective;
5. preserve both records.

## 8. Architecture changes

If implementation discovers that a durable invariant or ownership boundary must change, stop the implementation plan and record an ADR or canonical amendment first.

Examples requiring architecture review include:

- adding a global queue to eggworkd;
- automatic worker selection;
- changing transport-derived identity into payload identity;
- allowing stale generations to control newer work;
- changing required isolation into silent best-effort;
- making Git semantics part of Eggwork core;
- duplicating Eggfetch/Eggress/EggServe transport ownership;
- letting CodeGG move its scheduler authority into Eggwork.

## 9. External dependencies

Published versions and repository SHAs in plans are research baselines, not permanent pins unless a release plan explicitly says otherwise.

Implementation agents MUST re-check the current public interfaces before coding against Eggfetch, Eggress, EggServe/eggnet-tls, Eggup, Gregg, or CodeGG.

If a required interface does not exist, record a blocker or a narrowly scoped upstream prerequisite. Do not fork substantial upstream functionality into Eggwork merely to bypass the blocker.

## 10. Verification philosophy

Prefer deterministic local fixtures for routine behavior:

- local TLS CA/cert fixtures;
- local server/client loopback;
- local proxy fixtures;
- temporary workspaces/blob stores;
- controlled child processes;
- cancellation/timeout/process-tree fixtures.

Use hosted/live infrastructure only where the property being qualified genuinely requires it.

## 11. Planning registry

`plans/registry.md` is the active control surface.

Before handing off work:

- register the implementation plan;
- mark status accurately;
- list blockers;
- list immediately ready work;
- list external interface assumptions.

After closure:

- link the closure record;
- update the subsystem roadmap milestone;
- update the registry;
- unlock only directly satisfied dependents.
