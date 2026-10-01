# Operations and Distribution M002a — RecoveryRequired Restart Suppression and Eggup Lifecycle Composition Corrective

Status: closed

Finding origin:

- discovered during Operations M004 research after Operations M003 closure;
- affected historical milestone: Operations M002 — Eggup deployment and service integration;
- historical closure remains immutable at `plans/closure/operations-distribution/002-resumed-status.md`.

Reviewed Eggwork baseline:

- `4a5dd1cbab25e767162e2020bdb2126bc07b80c3`

Reviewed published Eggup baseline:

- tag `v0.1.1` -> `881c95ff069d3d465a282cb6a495ba6fcb70cb6f`;
- `eggup-core 0.1.1`;
- `eggup-service 0.1.1`.

Source roadmap:

- `plans/subsystems/operations-distribution-roadmap.md`

Primary class: corrective / recovery safety / lifecycle composition

Severity: medium correctness until closed.

## 1. Finding

Operations M002 correctly moved artifact commit/rollback and service-manager mechanics into Eggup, but Eggwork still manually sequences the service around the Core transaction.

The current `deployment::orchestrate_update` flow is:

1. validate candidate sources;
2. set persistent drain;
3. inspect and require `Owned`;
4. wait for active executions;
5. if previously running, call `manager.stop`;
6. call `commit_with_health_check`, which delegates artifact commit to
   `ValidatedTransaction::commit_with_post_commit`;
7. if previously running, unconditionally call `manager.start`;
8. return the Core `TransactionReceipt`.

The restart in step 7 is not conditioned on the terminal artifact disposition. In addition, the existing `commit_with_health_check` callback runs inside the Core transaction before Eggwork's step-7 service restart, so it cannot by itself prove post-start service health even though the historical closure described the broader boundary as post-update/post-start health. Historical evidence remains immutable; the corrective uses Eggup's lifecycle seam to put restoration and the post-install check in the correct order.

Eggup Core may return a successful Rust `Ok(TransactionReceipt)` whose
`TransactionDisposition` is `RecoveryRequired` when rollback cannot be
proven complete. In that state the artifact generation is uncertain and a
service MUST NOT be automatically started against it.

Published `eggup-service 0.1.1` already provides the correct composition:

- `commit_with_lifecycle`;
- `LifecycleUpdatePolicy`;
- `LifecycleUpdateReceipt`;
- `LifecycleRestorationStatus::NotAttemptedRecoveryRequired`.

Its documented contract explicitly states that `RecoveryRequired` suppresses
automatic service starts.

The historical M002 closure tests ordinary commit and verified rollback paths,
but contain no `RecoveryRequired` case and therefore did not prove this
terminal state.

No production incident is known. This is a latent recovery-path correctness
defect found by static contract review.

## 2. Objective

Remove Eggwork's duplicate stop/restart decision logic from the update
transaction and compose the already-published Eggup lifecycle transaction so
the following remain true for every terminal artifact disposition:

- `Committed`: restore the requested service state and run the bounded
  Eggwork post-install check;
- `RolledBack`: restore the exact pre-update service state;
- `RecoveryRequired`: perform no automatic start and expose manual recovery
  required;
- Core errors before a terminal receipt: restore prior lifecycle state only
  through Eggup's defined lifecycle error path;
- Foreign/Unknown/Absent registrations remain non-mutable.

Persistent drain and CodeGG/Eggwork execution quiescence remain Eggwork policy.
Artifact backup/rollback and service transition mechanics remain Eggup-owned.

## 3. API compatibility

Do not break the existing public
`orchestrate_update(...) -> Result<eggup_core::TransactionReceipt, DeploymentError>`
surface.

Add an internal or additive richer entry point, recommended:

`orchestrate_update_with_lifecycle(...)`

returning `eggup_service::LifecycleUpdateReceipt`.

Then implement the existing `orchestrate_update` compatibility surface as a
thin projection of the corrected lifecycle orchestration to
`receipt.transaction`.

The compatibility wrapper MUST NOT reintroduce stop/start calls.

Operations M004 may consume the richer receipt for machine-readable operator
evidence.

## 3a. Published generic seam compatibility

Published Eggup 0.1.1 declares:

`commit_with_lifecycle<M: ServiceManager, C: PostInstallCheck>(manager: &mut M, ...)`

so `M` is sized by default. Eggwork's existing public compatibility API accepts
`&mut dyn ServiceManager`.

Preserve that API by using a private sized forwarding adapter around the trait
object (or an equivalently non-breaking private mechanism) that implements
`ServiceManager` by delegating exactly:

- `inspect`;
- `install`;
- `uninstall`;
- `start`;
- `stop`;
- `restart`.

The adapter owns no policy, parsing, retries, timeouts, state, or manager
mechanics. It exists only to satisfy the published generic signature.

Do not change the public `orchestrate_update` parameter from a trait object to
a generic type merely to call Eggup; that would be a source/API compatibility
change.

## 4. Preparation ordering

Candidate work must complete before any lifecycle mutation.

Required order:

1. validate every `CandidateSource`;
2. build the `InstallPlan`;
3. `prepare()`;
4. `verify_integrity()`;
5. validate candidate set through the existing validators;
6. set/confirm persistent drain;
7. wait for Eggwork active-execution quiescence;
8. enter `eggup_service::commit_with_lifecycle`.

The resulting `ValidatedTransaction` is the object handed to Eggup service
composition.

Do not stop a service merely to discover that a candidate cannot be staged,
hashed, or validated.

## 5. Lifecycle policy mapping

Translate the existing Eggwork `UpdatePolicy` to
`eggup_service::LifecycleUpdatePolicy` without changing defaults:

- `restore = RestoreIntent::Preserve`;
- `post_commit_failure = request.policy.post_commit`;
- `quiesce_timeout`: bounded from the existing service transition policy;
- `post_commit_timeout`: bounded by the existing health timeout;
- `rollback_restore_timeout`: bounded and explicit.

The current default remains
`PostCommitFailurePolicy::RollBack`.

A service that was stopped before the update remains stopped under
`Preserve`; do not fabricate a start merely because an update succeeded.

## 6. Post-install check adapter

Adapt Eggwork's existing bounded health/version check to Eggup's
`PostInstallCheck` seam.

Requirements:

- honor the `remaining: Duration` budget supplied by Eggup;
- no network/release discovery is introduced by this corrective;
- error detail remains bounded and secret-safe;
- a check panic/failure is handled through Eggup's lifecycle receipt;
- Linux helper version/trust checks may be included by callers but this
  corrective does not expand platform policy.

If preserving the current `FnOnce` callback API requires an adapter, use a
single-consumption private wrapper. Do not run a caller check twice.

## 7. RecoveryRequired behavior

This is the controlling acceptance boundary.

When the Core receipt is `RecoveryRequired`:

- the richer lifecycle receipt reports
  `manual_artifact_recovery_required() == true`;
- restoration is
  `LifecycleRestorationStatus::NotAttemptedRecoveryRequired`;
- Eggwork performs no `start`, `restart`, service install/refresh, or
  implicit undrain after the receipt;
- the persistent drain marker remains set when the update path owns it;
- operator output in later M004 work must make manual recovery required
  explicit;
- recovery evidence/path from Core is preserved and not replaced by an
  Eggwork summary.

## 8. Tests

Add deterministic regression coverage for:

1. valid candidate preparation occurs before the first service mutation;
2. candidate validation/integrity failure leaves a running service untouched;
3. Owned+Running + successful commit -> running;
4. Owned+Stopped + successful commit + Preserve -> stopped;
5. post-install failure + RollBack -> old generation restored and prior
   lifecycle state restored;
6. post-install failure + KeepInstalled -> new generation remains, lifecycle
   failure remains visible;
7. `RecoveryRequired` -> zero automatic start/restart and drain remains;
8. Core pre-receipt failure -> prior service state restoration follows Eggup
   lifecycle semantics;
9. Foreign/Unknown/Absent -> zero lifecycle/artifact mutation;
10. compatibility `orchestrate_update` returns the exact underlying Core
    receipt without extra lifecycle mutation;
11. no direct manager invocation enters Eggwork;
12. existing M002 helper/version, drain, ownership, and rollback cases remain
    green.

If forcing a real Core `RecoveryRequired` is impractical through stable
public fault injection, add a narrow private disposition-to-operator-policy
test seam and combine it with the published Eggup 0.1.1
`commit_with_lifecycle` contract tests. Do not add production fault controls.

## 9. Verification

Minimum:

```bash
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
git diff --check
```

Additionally:

- inspect `cargo tree -p eggwork-server` and confirm the existing published
  Eggup 0.1.1 dependencies remain registry-backed;
- prove no new direct `systemctl`, `launchctl`, `sc.exe`, PowerShell
  service mutation, or shell lifecycle path exists;
- prove the compatibility wrapper contains no post-receipt start/restart.

Hosted qualification may use ordinary Linux CI because the corrective is
manager-neutral and deterministic through Eggup's test manager. Native manager
mutation belongs to M004.

## 10. Acceptance criteria

1. Eggwork no longer manually decides restart after Core terminal disposition.
2. `RecoveryRequired` can never trigger automatic service start.
3. Rolled-back updates restore the prior lifecycle state through Eggup, and the post-install check executes only after the success-state lifecycle restoration that Eggup owns.
4. Successful updates preserve stopped/running intent through
   `RestoreIntent::Preserve`.
5. Candidate preparation/integrity validation precedes service quiescence.
6. Persistent drain and active-execution policy remain Eggwork-owned.
7. Existing public `orchestrate_update` source surface is preserved.
8. No artifact rollback or manager mechanics are copied from Eggup.
9. Existing M002/M003/security tests remain green.
10. No unresolved high/medium finding remains in this corrective's scope.

## 11. Stop conditions

Stop and re-plan if:

- published Eggup 0.1.1 does not expose the reviewed
  `commit_with_lifecycle` behavior;
- adopting the lifecycle seam would require a breaking public Eggwork API;
- Eggwork must duplicate lifecycle restoration logic to preserve behavior;
- the lifecycle receipt loses Core recovery evidence;
- a `RecoveryRequired` path still reaches an Eggwork start/restart call.

## 12. Closure evidence

Create:

`plans/closure/operations-distribution/002a-status.md`

Record:

- exact Eggwork corrective implementation SHA;
- exact published Eggup 0.1.1/tag evidence;
- before/after orchestration authority map;
- preparation-before-quiesce proof;
- running/stopped/rollback/KeepInstalled matrix;
- explicit `RecoveryRequired` restart-suppression evidence;
- persistent-drain disposition;
- API compatibility evidence;
- ownership guard results;
- full regression/CI results;
- unresolved findings;
- M004 unblock disposition.

## 13. Dependency transition

Operations M004 is blocked on this corrective.

When M002a closes with no high/medium finding, M004 becomes ready to execute
against the corrected lifecycle composition. Historical M002 closure evidence
remains unchanged.
