# CodeGG Integration Roadmap

Status: active roadmap

Canonical authority:

- `plans/000-long-term-specification.md#20-codegg-integration`
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`
- CodeGG's own scheduler/AgentRun/worktree architecture remains authoritative for CodeGG.

Planning baseline reviewed:

- CodeGG commit `2f7d84f88070eee2a4fb9f70b6d7d5d10b01048d`

Implementation agents MUST re-check the current CodeGG head before coding.

## 1. Ownership boundary

Eggwork integration supplies a remote execution backend. CodeGG retains:

- JobStore / JobAttempt;
- JobScheduler and ResourcePermitGuard;
- AgentTask/AgentRun;
- WorkOrder;
- project/session principal and audit;
- WorktreeService and Git integration;
- semantic retry/placement policy;
- RunStore interpretation.

## 2. Existing CodeGG seams to consume

At the planning baseline:

- `JobExecutor` / `JobExecutionContext` provide typed executor dispatch.
- `JobSubmissionService` and `JobScheduler` own durable scheduling/admission.
- `ManagedProcessService` owns local finite argv lifecycle.
- `WorktreeService` owns managed worktree leases/generations.
- interactive-process protocol demonstrates bounded sequence/resync and transport-derived ownership.
- origin attribution/audit already distinguish durable caller provenance.

Eggwork should align with these seams rather than create a second CodeGG daemon scheduler.

## 3. Durable invariants

1. CodeGG selects the node before Eggwork invocation.
2. CodeGG scheduler permit remains held across remote execution until remote cleanup/terminal convergence.
3. One CodeGG attempt maps deterministically to one Eggwork execution identity/generation policy.
4. Remote busy/capability mismatch returns to CodeGG policy; Eggwork does not choose another node.
5. CodeGG cancellation reaches the remote execution.
6. CodeGG worktree ownership remains local/authoritative even if its content is snapshotted remotely.
7. Returned artifacts/Git evidence do not silently mutate/integrate the parent worktree.
8. AgentRun state remains CodeGG-owned.

## 4. Milestones

### M001 — Fixed-target remote executor adapter

Class: capability/invariant

Status: blocked on Eggwork Control Plane M002 and Workspace M003 closure

Implementation plan:

- `plans/implementation/codegg-integration/001-codegg-fixed-target-remote-executor.md`

Objective:

Add an Eggwork-backed CodeGG JobExecutor for finite build/test/lint/format/managed argv classes, with explicit node target, workspace transfer, progress, cancellation, artifacts, and terminal mapping.

Exit conditions:

- one scheduled CodeGG attempt produces one remote execution;
- duplicate transport retry cannot create a second remote child;
- scheduler remains sole CodeGG admission/fairness owner;
- remote result maps to existing completion semantics;
- local executor remains available;
- no CodeGG worktree/AgentRun authority moves into Eggwork.

### M002 — Remote-target policy and capability projection

Class: infrastructure/polish

Status: blocked on M001

Objective:

Expose node capability/status facts to CodeGG's own configuration/selection policy, diagnostics, and scheduler executor health without implementing selection inside Eggwork.

### M003 — Git-aware workspace adapter

Class: capability/infrastructure

Status: blocked on M001 and a separately reviewed Eggwork optimized materializer interface

Objective:

Reduce workspace transfer using repository commit/bundle/object/patch evidence while preserving CodeGG worktree identity and dirty state.

### M004 — Whole remote AgentRun worker

Class: capability

Status: blocked on M001-M003 and stable CodeGG AgentRun worker-entry contract

Objective:

Execute a bounded CodeGG worker process remotely under Eggwork while the central CodeGG daemon remains authoritative for the AgentRun.

Exit conditions:

- remote worker cannot self-create authoritative run identity;
- model/tool events map back under existing run ownership;
- cancellation and lease loss converge;
- returned commit/diff/artifacts are validated;
- parent integration remains explicit.

## 5. Initial mapping intent

```text
CodeGG JobId / AttemptId
      |
      | deterministic adapter identity
      v
Eggwork ExecutionId / Generation

CodeGG cancellation token ------> Eggwork cancel
Eggwork ExecutionEvents --------> JobProgressSink
Eggwork Artifact handles -------> RunStore import/reference
Eggwork ExecutionResult --------> ExecutorCompletion
```

The exact identity derivation must be collision-safe and persisted/inspectable; do not use display strings.

## 6. Verification strategy

- local-vs-remote result equivalence fixtures;
- CodeGG scheduler contention with remote work;
- remote busy/capability mismatch;
- duplicate HTTP/reconnect;
- cancellation at queued-before-remote, preparing, running, cleanup;
- CodeGG daemon restart with remote node still alive;
- Eggwork node restart with CodeGG attempt alive;
- worktree snapshot mutation isolation;
- artifact import;
- no second scheduling authority/static ownership review.
