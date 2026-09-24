# CodeGG Integration M001 Closure — Fixed-Target Remote Executor (Eggwork Reference Contract)

Status: closed (Eggwork substrate-side reference contract only)

Source implementation plan: `plans/implementation/codegg-integration/001-codegg-fixed-target-remote-executor.md`
Subsystem roadmap: `plans/subsystems/codegg-integration-roadmap.md`
Reviewed Eggwork head: `4efc91efa53da06663c9bf7d6a948fa240122454`
Planning baseline for CodeGG re-audit: planning head `5f4532659dbf0df2cd9f2b3bdb024217d2ea7868`, M001 implementation `67f8f3d33651846bbdbd3e4a3bc239e50a0237a6`, historical M001 closure `338074062e3e8748ba83708ea3c2a7e33de12f74`

## 1. Executive finding

M001 is closed on the Eggwork side as a reference-contract milestone with zero
production code changes. The re-audit confirms the Eggwork substrate already
exposes every contract surface the plan requires — fixed-target addressing with
no scheduler, deterministic `ExecutionId`/`ExecutionGeneration`/`LeaseId`
identity with fencing, workspace manifest/blob transfer with relative cwd and
symlink-safe semantics, bounded sequenced events, fenced cancellation,
declared artifacts, and typed busy/draining/capability-mismatch refusals — and
that all of it is covered by the already-closed Foundation M001-M003 (plus CI
corrective C001), Control Plane M001-M003 (plus lease-expiry correction),
Workspace/Artifact M001-M003, Security M001-M003 (plus Landlock `/dev/null`
corrective), and Operations M001 closures at the reviewed head.

No stop condition (§12) was triggered: no worker selection, no permit
semantics, no AgentRun/Worktree authority, no idempotency bypass, and no
absolute-path handling were added to Eggwork. Downstream CodeGG execution
authority is unchanged: CodeGG owns target selection, scheduler admission,
AgentRun/worktree state, and RunStore interpretation, and CodeGG's own
post-closure corrective C001
(`plans/implementation/eggwork-fixed-target-remote-execution-corrective/001-lease-identity-and-live-node-qualification.md`
in `dbowm91/codegg`) remains the controlling gate for current downstream
qualification. This closure does not qualify the CodeGG adapter itself and does
not unblock CodeGG M002/M003/M004.

## 2. CodeGG head re-audit (plan requirement)

Implementation agents were required to re-audit the current CodeGG head first.
Checked at closure time against the locally available `~/projects/codegg`
checkout:

- Planning head `5f453265` registers the post-closure corrective gate:
  M001 implementation/closure remain immutable historical evidence; C001
  (exact lease-token persistence + real NodeClient → mTLS → eggworkd
  qualification) is ready and M002/M003 stay blocked until it closes.
- M001 implementation `67f8f3d3` landed the scheduler-owned `EggworkExecutor`
  with durable `ExecutionTarget`, deterministic handle persistence, bounded
  workspace transfer, lease renewal, artifact import, and target-first routing
  with no local fallback.
- Historical closure `33807406` records M001 acceptance evidence (20
  `tests/eggwork_remote_execution.rs` tests via scripted client seam, 6
  `src/scheduler/eggwork.rs` unit tests, `scripts/verify.sh full` green).
- The post-closure finding that controls current qualification (not this
  Eggwork plan): `derive_handle` generated one lease token for the live
  Eggwork `ExecutionHandle` and a separate random token for the persisted
  `RemoteExecutionHandle`, while Eggwork cancel/renew fences on the accepted
  lease token; and M001 qualified restart behavior only through scripted
  clients, not the production mTLS node boundary. The fix belongs downstream
  in CodeGG C001. Eggwork lease-fencing behavior (Control Plane M002 closure)
  is the intended contract and is unchanged by this closure.

## 3. Requirement-to-evidence matrix (plan §11 acceptance criteria, Eggwork side)

| # | Acceptance criterion | Eggwork contract evidence | Result |
|---|---|---|---|
| 1 | CodeGG can intentionally target one Eggwork node | Eggwork performs no node selection: `eggwork-client` targets one explicit caller-selected node endpoint; the server exposes no queue, DAG, priority, or placement policy. Local admission only (`max_active_executions` semaphore in `crates/eggwork-server/src/lib.rs`, persistent drain marker in `crates/eggwork-server/src/operations.rs`). | pass |
| 2 | Scheduler remains sole global admission/fairness authority | Eggwork adds no scheduler: saturation and drain return typed refusals (`rejected_busy`, `rejected_draining`, `rejected_capability` counters in `crates/eggwork-server/src/operations.rs`); the caller decides wait/retry/select-another/fail. No cross-node state exists. | pass |
| 3 | One attempt maps to one remote execution identity | Deterministic fenced identity `ExecutionHandle { execution_id, generation, lease_id }` (`crates/eggwork-core/src/lib.rs`); reserve-before-spawn with transactional conflict on spec/principal/lease mismatch; generation must be exactly latest+1 with stale generations fenced (409); single lease token is bearer authority and redacted in `Debug`. Covered by Control Plane M002 closure (`plans/closure/control-plane-protocol/002-status.md`). | pass |
| 4 | Cancellation and progress propagate | Fenced cancel/renew bound to latest generation + lease hash; bounded sequenced event journal (256 events / 512 KiB, typed 410 history-expired / 416 cursor-ahead) with live broadcast; client `observe`/`observe_generation`/event-stream APIs in `crates/eggwork-client/src/lib.rs`; daemon restart conservatively interrupts uncertain nonterminal work without rerun. Covered by Control Plane M002 closure. | pass |
| 5 | Local worktree is not remotely mutated in place | Content-addressed manifest/blob contract with isolated materialization and relative cwd; symlink/path semantics follow the Eggwork contract; transfer policy preserves only required files per explicit caller policy. Covered by Workspace M001-M003 closures (`plans/closure/workspace-artifact-transport/001-status.md`, `002-status.md`, `003-status.md`). | pass |
| 6 | Artifacts/result map to existing CodeGG domain | Declared outputs return typed `ArtifactRecord { artifact_id, execution_id, generation, path, kind, digest, size_bytes, ... }` (`crates/eggwork-core/src/lib.rs`) with retention/GC bounds; terminal `ExecutionResult` is single-assigned. The mapping into CodeGG RunStore is downstream CodeGG-owned. Covered by Workspace M003 closure. | pass |
| 7 | Eggwork busy/unsupported does not choose another node | Typed refusal facts only: `NodeCapabilities { protocol min/max, features, max_active_executions }` with validation, `NodeStatus { draining, active_executions, capabilities }`, and `rejected_busy`/`rejected_draining`/`rejected_capability` metrics. No retarget/retry path exists in Eggwork; reconnect to the same accepted execution under idempotency/lease semantics is the only allowed retry shape. | pass |
| 8 | AgentRun/worktree ownership stays in CodeGG | Eggwork has no AgentRun/Worktree concepts: the durable store keeps only request digest, principal attribution, hashed lease, and snapshots — not runs, sessions, or worktrees. Authorization/origin attribution is preserved per Security M001 closure. No CodeGG authority moved. | pass |

Eligible job classes (§7: build/lint/format-check/test/managed argv) and
busy/capability/transport policy (§8) are CodeGG adapter decisions consuming
the contract above; interactive PTYs and provider/model loops remain
explicitly out of scope and were not moved. Cancellation/restart cases (§9)
are covered substrate-side by the Control Plane M002 restart-recovery,
lease-expiry, duplicate-submission, and generation-fencing evidence cited
above; CodeGG-attempt-level cases (queued-before-submit, daemon restart with
remote alive, partition shorter/longer than lease, double terminal delivery)
are downstream CodeGG adapter evidence, owned by CodeGG M001/C001 — not
re-qualified here.

## 4. Verification actually run

Host: Linux x86_64; Rust 1.89.0.

- `rtk cargo test --workspace --all-targets` — 82 passed, 1 ignored across 7 suites.
- `rtk cargo clippy --workspace --all-targets -- -D warnings` — passed (no issues found).
- `cargo fmt --all -- --check` — passed (clean).
- `rtk git diff --check` — passed.
- Code inspection (not executed qualification): fixed-target shape
  (`crates/eggwork-client/src/lib.rs`, `crates/eggwork-server/src/lib.rs`,
  `crates/eggwork-server/src/operations.rs`), identity/lease/fencing types
  (`crates/eggwork-core/src/lib.rs`), and prior closure records listed in §3.
- No new Eggwork code was written for this milestone, so there are no new
  focused tests to run; the contract is exercised by the existing closed
  milestone suites re-run above.

## 5. Compatibility, security, and platform review

- Only the Linux host loopback/process path is qualified by the re-run above,
  consistent with the underlying milestone closures. Windows/macOS claims
  require hosted runtime evidence; cross-compilation is not qualification.
- Lease credentials remain bearer tokens stored durably only as SHA-256 hash
  with `Debug` redaction; API errors are fixed and bounded (Control Plane
  M002, Security M001 evidence — unchanged).
- No compatibility or migration surface changes: no config version bump, no
  schema change, no new endpoint, no dependency change.
- No architecture-review trigger (§8 of the planning process): no queue,
  selection, identity, isolation, Git, transport-duplication, or scheduler
  boundary change was made.

## 6. Unresolved findings

- None on the Eggwork side. Severity high/medium: none.
- Known downstream gate (not an Eggwork finding): CodeGG corrective C001
  (divergent live/persisted lease tokens; missing real-node mTLS
  qualification) controls current downstream qualification. Tracked in CodeGG,
  mirrored in `plans/registry.md` and
  `plans/subsystems/codegg-integration-roadmap.md`.

## 7. Disposition

- **Closed** as an Eggwork substrate-side reference-contract milestone at the
  reviewed head. The Eggwork integration-contract evidence is this record plus
  the cited closed milestone closures; no Eggwork production change was
  required.
- Downstream CodeGG M001 implementation/closure remain immutable historical
  records in CodeGG; current downstream qualification is gated by CodeGG C001.
- Registry/roadmap disposition: record this closure, mark the Eggwork
  reference plan closed, keep CodeGG M002/M003/M004 blocked on downstream
  C001, and unlock no further dependents (see next-plan review below).

## 8. Next-plan dependency review (2026-09-24)

- CodeGG M002 (capability/target projection): remains **blocked** on
  downstream CodeGG corrective C001. This closure does not satisfy that gate.
- CodeGG M003 (Git-aware materializer): remains **blocked** on C001 plus a
  separately reviewed optimized Eggwork materializer interface. Unchanged.
- CodeGG M004 (remote AgentRun worker): remains **blocked** on C001 +
  M002-M003 + stable worker-entry contract. Unchanged.
- Security M004: remains **blocked** on Operations M002. Unchanged; this
  closure contributes no new security surface.
- Operations M002: remains **ready** (independent; not a dependency of this
  plan). Unchanged.
- Operations M003/M004, reverse-connect, PTY: unchanged (blocked/deferred as
  registered).
