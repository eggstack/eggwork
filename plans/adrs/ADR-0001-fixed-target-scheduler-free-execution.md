# ADR-0001: Fixed-Target, Scheduler-Free Remote Execution

Status: accepted

Date: 2026-09-22

Decision owners: project maintainers

Related specification:

- `plans/000-long-term-specification.md#1-product-definition`
- `plans/000-long-term-specification.md#3-non-goals`
- `plans/000-long-term-specification.md#4-architectural-principles`
- `plans/001-terminology-and-domain-model.md`
- `plans/002-long-term-roadmap.md`

## Context

Eggwork is intended to make remote process execution reusable across Eggstack and especially useful to CodeGG. The main architectural risk is reproducing a scheduler inside the execution fabric.

CodeGG already has a mature durable scheduler with admission, fairness, cancellation, job/attempt identity, recovery, AgentRun integration, and worktree ownership. Other consumers may have their own schedulers or may simply select a machine manually. A second global queue or placement engine inside Eggwork would duplicate policy, produce conflicting ownership, and make Eggwork less reusable.

At the same time, a remote execution node cannot accept unlimited work. It needs finite concurrency, storage, output, connection, and isolation bounds.

## Decision drivers

- CodeGG must retain one global scheduler authority.
- Eggwork must be reusable without CodeGG.
- Node safety requires local admission.
- Saturation must be explicit rather than hidden behind an internal queue.
- Callers need stable facts to implement their own placement/retry policy.
- The core API should remain understandable as "execute on this node."

## Considered options

### Option A — Eggwork cluster scheduler

Eggwork would maintain worker discovery, a central queue, priorities, fairness, placement, and retries.

Rejected. It duplicates CodeGG and turns a generic execution substrate into an orchestration product.

### Option B — Node-local scheduler with queues and priorities

Each eggworkd would accept requests into a local queue and reorder them according to priority/fairness.

Rejected as the default model. Even though placement stays caller-owned, queued priority policy still competes with the caller's scheduler and makes cancellation/lease semantics harder to reason about.

### Option C — Fixed-target execution with protective local admission

The caller selects one node. That node validates the request and either accepts it immediately or returns a typed refusal such as busy, draining, unauthorized, or capability mismatch.

Selected.

## Decision

1. The public client API is fixed-target. A caller addresses one node or one already-established node connection.
2. Eggwork has no cluster-wide submission API, placement API, worker-selection policy, fairness policy, workflow DAG, or semantic retry engine.
3. Each node owns protective local admission with explicit hard bounds.
4. Local admission may reject but does not build an unbounded policy queue.
5. A busy response may carry bounded retry timing advice, but retry/alternate-node selection is caller policy.
6. Node capability and status APIs expose facts for caller schedulers without interpreting those facts into placement.
7. CodeGG integration occurs at CodeGG's executor boundary after CodeGG scheduling/admission has already selected the remote target.
8. Reverse-connect infrastructure, if added, is transport only and must not gain placement/scheduling authority.

## Consequences

### Positive

- CodeGG keeps a single scheduling authority.
- Eggwork stays useful to simple CLI users and other schedulers.
- Saturation and placement failure remain observable.
- Nodes remain independently operable.
- Testing is simpler because one request has one selected destination.

### Negative

- Multi-node consumers must implement node selection themselves.
- Eggwork cannot provide transparent cluster balancing.
- A caller that wants queued semantics must own that queue.

### Deferred

A separate project could eventually provide scheduling above Eggwork, but it must consume Eggwork as an execution backend rather than change this boundary.

## Verification

Conforming implementations must prove:

- no server endpoint accepts "any node" or cluster placement requests;
- no daemon queue reorders work by application priority;
- admission saturation returns a typed refusal without child-process creation;
- CodeGG remote execution retains CodeGG JobScheduler ownership;
- proxy/reverse-connect routing does not become placement policy.

## Supersession

None.
