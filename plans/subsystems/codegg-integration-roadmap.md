# CodeGG Integration Roadmap

Status: active roadmap

Canonical authority:

- `plans/000-long-term-specification.md#20-codegg-integration`
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0004-content-addressed-workspaces-and-artifacts.md`
- CodeGG's own scheduler/AgentRun/worktree architecture remains authoritative for CodeGG.

Planning baseline reviewed:

- CodeGG current reviewed head `841ad117399c9279e3668828d5d9866983603381`
- CodeGG remote-execution M002a closure head `f5f8d96d7b7371c8583196c58d36ef7b3118ed3c`
- CodeGG M001 implementation `67f8f3d33651846bbdbd3e4a3bc239e50a0237a6`
- CodeGG historical M001 closure `338074062e3e8748ba83708ea3c2a7e33de12f74`

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

Status: Eggwork reference contract closed; downstream CodeGG M001+C001+M002+M002a are closed

Eggwork integration-contract plan:

- `plans/implementation/codegg-integration/001-codegg-fixed-target-remote-executor.md`

Controlling downstream records in CodeGG:

- M001 implementation: `67f8f3d33651846bbdbd3e4a3bc239e50a0237a6`
- historical closure: `338074062e3e8748ba83708ea3c2a7e33de12f74`
- CodeGG corrective C001 closure: `d3d390d56620fb5c6f755a5dfb0987e0a1c01651`
- CodeGG M002 closure: `plans/closure/eggwork-fixed-target-remote-execution/002-status.md`
- CodeGG M002a closure/head: `f5f8d96d7b7371c8583196c58d36ef7b3118ed3c`
- roadmap: `plans/subsystems/eggwork-fixed-target-remote-execution-roadmap.md`
- post-closure corrective: `plans/subsystems/eggwork-fixed-target-remote-execution-post-closure-corrective-addendum.md`
- corrective C001: `plans/implementation/eggwork-fixed-target-remote-execution-corrective/001-lease-identity-and-live-node-qualification.md`

CodeGG corrective C001 closed the lease-identity/mTLS gap; Eggwork remote-admission C001 then closed the restricted-isolation substrate gap; CodeGG M002 and M002a subsequently closed against the corrected immutable Eggwork pin with required-Landlock live qualification. This document remains substrate-side reference authority only. The Eggwork reference-contract milestone itself is closed with contract evidence in `plans/closure/codegg-integration/001-status.md`. M003 is now the next integration line and is blocked specifically on Eggwork Workspace/Artifact M004; M004 remains deferred on M003 plus a stable AgentRun worker-entry contract.

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

Status: closed downstream, including M002a restricted-spec live requalification

Controlling downstream evidence:

- `dbowm91/codegg: plans/closure/eggwork-fixed-target-remote-execution/002-status.md`
- `dbowm91/codegg: plans/closure/eggwork-fixed-target-remote-execution/002a-status.md`
- M002a closure/head `f5f8d96d7b7371c8583196c58d36ef7b3118ed3c`.

Objective:

Expose node capability/status facts to CodeGG's own configuration/selection policy, diagnostics, and scheduler executor health without implementing selection inside Eggwork.

### M003 — Content-aware derived workspace transfer

Class: capability/infrastructure

Status: downstream implementation plan registered; blocked on Eggwork Workspace/Artifact M004 closure

Eggwork upstream prerequisite:

- `plans/implementation/workspace-artifact-transport/004-reusable-manifest-cas-and-derived-materialization.md`
- planning commit `ed0fc838bd006103021f1fb4749f579cabc2db87`

CodeGG downstream plan:

- `dbowm91/codegg: plans/implementation/eggwork-fixed-target-remote-execution/003-content-aware-derived-workspace-transfer.md`
- planning commit `9c96e0779d8092998958bbf7f22fec0f844339cf`

Objective:

Reduce repeated full-manifest transfer through Eggwork's Git-neutral retained-manifest/patch contract while preserving CodeGG's full local snapshot, exact-source provenance, worktree identity, and dirty-state semantics.

### M004 — Whole remote AgentRun worker

Class: capability

Status: deferred; M001+C001+M002+M002a are closed, but M003 and a stable CodeGG AgentRun worker-entry contract remain unresolved

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
