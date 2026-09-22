# CodeGG Integration M001 — Fixed-Target Remote Executor

Status: blocked on Eggwork Control Plane M002 and Workspace/Artifact M003 closure

Source roadmap:

- `plans/subsystems/codegg-integration-roadmap.md`

CodeGG planning baseline:

- `2f7d84f88070eee2a4fb9f70b6d7d5d10b01048d`

Implementation MUST re-audit the current CodeGG head first.

## 1. Objective

Integrate Eggwork as a CodeGG remote execution backend for scheduler-owned finite command classes without adding a second scheduler or moving AgentRun/worktree authority.

## 2. CodeGG baseline invariants

Preserve:

- JobSubmissionService before execution;
- JobScheduler as sole CodeGG admission/fairness authority;
- JobExecutionContext attempt identity/cancellation/permit lifetime;
- ManagedProcessService as local executor;
- WorktreeService as worktree owner;
- AgentRunStore/Service as agent-run owner;
- RunStore/artifact ownership;
- authorization/origin attribution.

## 3. Target selection

CodeGG must have an explicit already-resolved execution target in job/executor context or executor configuration.

Allowed conceptual values:

- local;
- one selected Eggwork NodeId/connection.

This milestone does not define automatic cluster scheduling. Any selection policy lives in CodeGG.

## 4. Deterministic remote identity

Define a persisted deterministic mapping from CodeGG JobId/AttemptId (and daemon generation where needed) to Eggwork ExecutionId/ExecutionGeneration.

Requirements:

- transport retry reuses the same identity;
- retry/new CodeGG attempt does not collide with prior execution;
- identity can be inspected in both systems;
- no display-title/string concatenation with ambiguous parsing.

## 5. Workspace transfer

For eligible finite jobs:

- snapshot the effective CodeGG workspace/worktree root through Eggwork manifest/blob contract;
- preserve only required files according to explicit policy;
- no mutation of original local worktree from remote execution;
- remote cwd is relative;
- symlink/path semantics follow Eggwork contract.

Initial implementation may transfer a full bounded workspace snapshot; Git-aware optimization is later.

## 6. Executor adapter

Implement Eggwork-backed JobExecutor/adapter that:

1. receives scheduler-owned JobExecutionContext;
2. resolves configured node connection;
3. verifies required capabilities;
4. uploads missing workspace blobs/materializes workspace;
5. submits one deterministic Eggwork execution;
6. maps events into JobProgressSink/bounded CodeGG progress;
7. propagates cancellation;
8. imports/references declared artifacts into CodeGG RunStore;
9. maps terminal facts into ExecutorCompletion;
10. returns only after remote cleanup/terminal convergence or explicit lost/interrupted classification.

The CodeGG ResourcePermitGuard remains held through this lifetime.

## 7. Eligible initial job classes

Prefer deterministic finite command classes already represented through canonical process/test runner semantics, such as:

- build;
- lint;
- format/check-only where safe;
- test;
- managed argv/tool programs only after their workspace/artifact requirements are understood.

Do not move interactive PTYs or provider/model loops in this milestone.

## 8. Busy/capability/transport policy

Eggwork returns facts:

- busy;
- draining;
- capability mismatch;
- transport unavailable;
- execution failure.

CodeGG decides whether/how a later attempt selects another node. The adapter must not secretly retry on another target.

Transport reconnect to the same accepted execution is allowed under Eggwork idempotency/lease semantics.

## 9. Cancellation/restart

Test:

- CodeGG queued cancellation before remote submit;
- cancel while remote preparing;
- cancel running;
- cancel during artifact capture;
- CodeGG daemon restart while Eggwork execution remains alive;
- Eggwork daemon restart while CodeGG attempt waits;
- network partition shorter/longer than lease;
- terminal result received twice/replayed.

Define conservative interrupted/lost behavior rather than fabricating success.

## 10. Tests

- local vs remote echo/build/test fixture;
- exact one CodeGG attempt -> one Eggwork execution;
- duplicate submission;
- busy;
- capability mismatch;
- remote nonzero/timeout/cancel;
- workspace mutation stays remote;
- artifacts returned;
- scheduler process slot/permit contention includes remote executor lifetime;
- no second scheduler/static architecture review.

## 11. Acceptance criteria

1. CodeGG can intentionally target one Eggwork node.
2. CodeGG scheduler remains sole global admission/fairness authority.
3. One attempt maps to one remote execution identity.
4. Cancellation and progress propagate.
5. Local worktree is not remotely mutated in place.
6. Artifacts/result map to existing CodeGG domain.
7. Eggwork busy/unsupported does not choose another node.
8. AgentRun/worktree ownership stays in CodeGG.

## 12. Stop conditions

Stop if integration requires:

- Eggwork selecting a worker;
- releasing CodeGG scheduler permit while remote process still runs;
- making Eggwork authoritative for AgentRun/Worktree;
- bypassing Eggwork idempotency;
- direct remote absolute paths instead of workspace contract.

## 13. Closure evidence

The Eggwork repo should record its integration contract evidence; CodeGG should separately record implementation/closure under CodeGG's own planning system for the actual downstream change.
