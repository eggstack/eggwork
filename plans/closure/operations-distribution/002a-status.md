# Operations M002a — RecoveryRequired Restart Suppression and Lifecycle Composition

Status: closed

Reviewed baseline: `4a5dd1cbab25e767162e2020bdb2126bc07b80c3`

Implementation commit: `c33e9d6fded7a80a377d6e6748cc9cdb2b02acbc`

## Requirement-to-evidence matrix

| Requirement | Evidence | Result |
|---|---|---|
| Replace Eggwork stop/restart sequencing with Eggup lifecycle composition | `deployment::orchestrate_update_with_lifecycle` calls published `eggup_service::commit_with_lifecycle`; old public function projects `receipt.transaction` only | Pass |
| Preserve trait-object public API | Public function still accepts `&mut dyn ServiceManager`; private `ServiceManagerAdapter` forwards exactly all six methods to satisfy Eggup's sized generic bound | Pass |
| Prepare before drain/quiescence/lifecycle mutation | Every source validates, then `build_install_plan`, `prepare`, `verify_integrity`, and candidate validation complete before drain and `wait_for_quiescence`; Eggup then inspects/quiesces | Pass |
| Preserve Eggwork policy and Eggup ownership | Persistent drain and execution quiescence remain Eggwork-owned; Eggup owns service transitions and artifact rollback; `RestoreIntent::Preserve`, existing post-commit policy, bounded 30-second transitions, and caller health budget are supplied | Pass |
| Preserve one-shot bounded health callback | `OneShotHealthCheck` consumes the existing `FnOnce` at most once, refuses an exhausted budget, and forwards bounded error handling to Eggup | Pass |
| Suppress restoration after RecoveryRequired | Eggwork contains no post-receipt start/restart branch. Eggup 0.1.1 source and its `recovery_required_suppresses_automatic_old_generation_restart` contract test retain the Core disposition, report manual recovery, and set `NotAttemptedRecoveryRequired` without restarting the old generation | Pass; lifecycle decision delegated to Eggup |
| Preserve Core recovery evidence | Rich receipt exposes Eggup's exact `transaction`; compatibility API returns that exact Core transaction receipt | Pass |
| Dependencies remain published | `cargo tree -p eggwork-server` shows registry `eggup-core v0.1.1` and `eggup-service v0.1.1`; Cargo.lock records registry checksums | Pass |
| No direct manager implementation | Manager mutations are only trait forwarding and Eggup adapter construction; static ownership guard passed | Pass |

The Eggup recovery contract can start the candidate generation while running
its post-install check before Core determines that rollback itself is
unrecoverable. On `RecoveryRequired`, Eggup suppresses restoration of the old
generation; Eggwork makes no additional lifecycle call. This is the published
Eggup contract and preserves the exact Core evidence for operator recovery.

## Verification actually executed

- `python3 scripts/check_execution_ownership.py` — passed.
- `python3 scripts/check_execution_ownership.py --prove-negative-exit` — passed.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — 165 passed, 1 ignored.
- `cargo check --workspace` — passed.
- `git diff --check` — passed.
- Focused `cargo test -p eggwork-server deployment::tests` — 18 passed.
- `cargo tree -p eggwork-server` — confirmed registry-backed Eggup 0.1.1 dependencies.
- Source inspection confirmed no service command execution is added to Eggwork.

## Findings and disposition

No unresolved high/medium finding remains in M002a scope. M004 is directly
unblocked and is being advanced as the next registered Operations milestone.
The historical M002 closure records remain unchanged.
