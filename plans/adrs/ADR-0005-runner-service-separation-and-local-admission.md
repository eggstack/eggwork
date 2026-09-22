# ADR-0005: Runner/Service Separation and Protective Local Admission

Status: accepted

Date: 2026-09-22

Decision owners: project maintainers

Related specification:

- `plans/000-long-term-specification.md#4-architectural-principles`
- `plans/000-long-term-specification.md#12-local-admission`
- `plans/000-long-term-specification.md#14-isolation-and-resource-enforcement`

## Context

The network-facing node service and the child-process executor have different responsibilities and risk profiles.

CodeGG already contains a mature finite-process owner whose useful semantics should be generalized rather than rewritten: sanitized argv execution, bounded output, cancellation/timeout, Unix process-session cleanup, sandbox outcome, and cleanup diagnostics.

Buildbarn provides a useful architectural precedent by separating a network/orchestration worker from a smaller runner boundary. Eggwork does not need two OS processes immediately, but it benefits from that ownership separation from the first implementation.

The node also needs concurrency protection. That protection must not grow into a scheduler.

## Decision drivers

- one process-spawn owner;
- testable local runner independent of HTTP;
- ability to privilege-separate later;
- explicit isolation/resource handoff;
- hard node bounds without queue policy;
- CodeGG/local embedding reuse;
- prevent server handlers from spawning ad hoc processes.

## Decision

1. `eggwork-runner` owns finite noninteractive process lifecycle.
2. `eggwork-server` owns authenticated protocol, authorization, validation, local admission, registry/lease/event/workspace coordination, and invokes the runner.
3. Server request handlers MUST NOT directly construct production child processes.
4. The runner MUST remain usable without the network server for deterministic tests and future local embedding.
5. The runner interface accepts validated execution-local inputs and returns/streams typed lifecycle facts; it does not know scheduler priority or node selection.
6. Local admission is server/node policy applied before runner invocation.
7. Initial admission is bounded immediate acceptance/rejection, not a priority queue.
8. Resource permits are held for the actual owned lifetime and released exactly once after runner cleanup converges.
9. The internal runner/service API must be designed so the runner could move to a helper process later without changing the public execution protocol.
10. Sandbox helper and resource-controller helper boundaries remain beneath the runner and must fail closed when required.
11. A static source guard SHOULD eventually reject production process spawning outside approved runner/platform helper modules.

## Consequences

### Positive

- one auditable execution owner;
- easy unit testing without TLS/network fixtures;
- future privilege separation remains possible;
- node admission and process cleanup lifetimes are explicit;
- CodeGG can reuse equivalent local execution semantics.

### Negative

- requires adapter types between protocol and runner;
- some data must be translated rather than sharing server internals directly;
- future two-process split may still require explicit IPC work.

## Local admission contract

The initial local admission layer may account for:

- active process slots;
- preparing/materializing slots;
- node drain state;
- storage headroom;
- request/output/artifact hard limits;
- requested enforceable capabilities.

It returns either an owned permit or a typed refusal.

It does not:

- reorder accepted work;
- implement user/project fairness;
- persist a scheduler queue;
- choose another node;
- retry automatically.

## Verification

- source guard or equivalent proves no normal server handler spawns directly;
- permit is acquired before side-effectful prepare/spawn and released once;
- spawn failure releases permit;
- cancellation/timeout hold permit through descendant cleanup;
- saturation returns busy with zero child process;
- runner unit tests operate without server;
- server integration tests use the same runner implementation;
- required sandbox/resource setup failure cannot bypass into direct execution.

## Supersession

None.
