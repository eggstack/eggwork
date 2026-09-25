# Operations M002 — Eggup Deployment and Service Integration

Status: ready for handoff

Previously blocked: Eggup revision `66813b3b94de3a9b2f270e0000dc339ef6f0b478` did not retain backups through post-start health validation. That blocker is historical and remains recorded in `plans/closure/operations-distribution/002-status.md`.

Unblock evidence: Eggup Verified Update Core M007 implemented `ValidatedTransaction::commit_with_post_commit` with `PostCommitFailurePolicy::{KeepInstalled, RollBack}` at implementation `8d5fc12f7224145285f22d0975a7bb91e1e363ea` and closed/qualified it at `2cab1f97ef30fa347c2030da321462459672c521`. The callback executes while the mutation lock and backup set remain owned, which satisfies this milestone's required bounded post-start health check/rollback boundary.

Source roadmap:

- `plans/subsystems/operations-distribution-roadmap.md`

Canonical references:

- `plans/000-long-term-specification.md#21-platform-and-packaging-targets`
- `plans/closure/operations-distribution/001-status.md`

Boundary references:

- Eggup owns consumer-side verified deployment, rollback, install receipts, and service lifecycle.
- Eggpack owns producer-side release construction, release manifests/evidence, installer generation, and generated release CI.

Current upstream baselines reviewed:

- Eggup M007 closure head `2cab1f97ef30fa347c2030da321462459672c521`;
- Eggup M007 implementation `8d5fc12f7224145285f22d0975a7bb91e1e363ea`;
- Eggup service M004 closure baseline `d894ae63a8963914e545a6d93dc3db92b998138c`;
- Eggpack current reviewed line after ReleaseManifest M001/M001a closure.

The current Eggup source includes Unix manager mechanics, a native Windows SCM adapter, and the post-commit rollback seam required by this plan. Implementation must re-check the exact API at handoff and use a published release when available or an exact immutable Git revision when publication lags.

## 1. Objective

Integrate Eggwork's installed-node lifecycle with Eggup's consumer deployment and service-management interfaces without duplicating updater or service-manager mechanics.

This milestone does not own producer release construction. It may define the Eggwork-side artifact set and consumer policy needed for installation, but Eggpack remains responsible for future producer manifests/build/CI/bootstrap generation.

## 2. Upstream dependency policy

At implementation start, determine whether the required corrected Eggup service API is available in a published crate release.

If published, use the released version.

If not yet published, an exact immutable Git revision MAY be used for integration qualification, provided:

- Cargo.lock captures the revision;
- no floating branch dependency is accepted;
- closure records that packaging/publication of Eggwork is still gated on a published or otherwise intentionally vendored/reproducible dependency policy.

Do not regress to the older crates.io 0.1.0 behavior if it lacks the reviewed adapters.

## 3. Eggwork install unit

Define the consumer deployment unit explicitly.

Expected files may include:

- `eggworkd`;
- any user-facing `eggwork` client binary if/when present;
- `eggwork-sandbox-helper` on Linux where required;
- default/example config only if installation ownership is clear;
- notices/licenses;
- version/provenance metadata.

Do not let helper/executable versions drift independently inside one installation transaction.

Use Eggup multi-artifact transaction semantics where available.

## 4. Service specification

Construct Eggup `ServiceSpec` or current equivalent from validated Eggwork configuration.

Ownership identity must include:

- exact installed eggworkd executable;
- critical argv/config path;
- stable service identity;
- any manager-specific service dependencies required by the chosen platform.

Never treat same-name/different-executable as owned.

Foreign or unknown registration must fail closed before destructive stop/reinstall/uninstall.

## 5. Manager adapters

Use Eggup's platform adapters rather than invoking managers directly from Eggwork.

Qualify as available:

- systemd manager mechanics on Linux;
- launchd on macOS;
- Windows SCM on Windows;
- cron/non-systemd fallback only if the current Eggup contract explicitly supports the intended lifecycle semantics.

Eggwork must not shell out to `systemctl`, `launchctl`, `sc.exe`, PowerShell service commands, or `crontab` as a parallel implementation.

Platform capability remains explicit; unsupported manager environments receive structured diagnostics.

## 6. Update/drain orchestration

Eggwork owns application policy around when a node may be replaced.

Implement a thin orchestration layer:

1. validate candidate/install transaction;
2. inspect current node/service ownership;
3. request persistent drain;
4. wait for active executions according to bounded policy, or require explicit force;
5. stop only an owned service;
6. perform Eggup transactional replacement;
7. start/restart owned service;
8. run bounded post-update health/version check;
9. invoke Eggup's post-commit check boundary so the bounded health/version probe runs while backups remain retained; select `RollBack` on failure unless an explicitly documented operator policy chooses `KeepInstalled`;
10. preserve/inspect execution recovery state after restart.

Eggup owns filesystem transaction/rollback mechanics. Eggwork owns the decision that remote execution should be drained before replacement.

Do not silently terminate arbitrary active work.

## 7. Helper/version compatibility

Define a compatibility check between eggworkd and the sandbox helper.

At minimum:

- detect missing helper;
- detect obvious version/protocol mismatch;
- fail required sandbox execution closed;
- make upgrade transaction replace mutually dependent binaries together.

Do not add a self-update protocol to the runner/helper.

## 8. Operator commands

Extend the existing operator CLI only where needed for the consumer lifecycle, for example:

- install/service status;
- service install/uninstall/start/stop/restart;
- update candidate/apply/rollback entry points if Eggwork is intended to expose them directly.

Keep machine-readable output.

A separate bootstrap installer generator is not part of this milestone; that belongs to Eggpack.

## 9. Tests

Use Eggup test doubles for deterministic ownership/transition cases and platform-hosted integration where manager behavior is claimed.

Required cases:

- owned install;
- foreign same-name registration;
- unknown/malformed registration;
- start/stop/restart idempotency;
- drain with active execution;
- update after drain;
- force policy explicit;
- candidate validation failure before stop;
- replacement failure and rollback;
- post-start health failure and rollback policy;
- daemon restart recovery after successful update;
- helper version skew;
- Linux systemd hosted test;
- macOS launchd hosted test when claimed;
- Windows SCM hosted test when claimed.

Do not claim runtime support from cross-compilation alone.

## 10. Producer packaging boundary

This milestone may add local fixture artifacts needed to exercise Eggup, but MUST NOT:

- invent the canonical ReleaseManifest;
- generate final release CI;
- own target/artifact naming contracts already assigned to Eggpack;
- generate bootstrap installers;
- publish releases automatically.

Register a separate Operations M003 only when the necessary Eggpack manifest/build interfaces are closed and concrete.

## 11. Acceptance criteria

1. Eggwork uses Eggup for consumer transaction/rollback/service lifecycle.
2. Eggwork directly invokes no platform service manager.
3. Foreign/unknown service registrations fail closed.
4. Multi-binary installation is transactionally coherent.
5. Drain/update/restart semantics preserve execution truthfulness.
6. Helper/version skew is detected.
7. Platform manager claims have hosted evidence.
8. Producer release authority remains in Eggpack.
9. No unresolved high/medium finding remains.

## 12. Stop conditions

Stop if:

- the required Eggup adapter is absent at the exact implementation baseline;
- integration would require copying Eggup manager/update logic;
- producer packaging requires inventing an Eggpack schema prematurely;
- update safety requires fabricating completion for active executions;
- service ownership cannot distinguish foreign registration.

## 13. Closure evidence

Create `plans/closure/operations-distribution/002-resumed-status.md` containing:

The existing `plans/closure/operations-distribution/002-status.md` is the immutable historical blocker record for the pre-M007 Eggup interface. Do not overwrite, rename, or rewrite it. The resumed closure record is the current success/blocked disposition for this re-opened milestone.

- exact Eggup revision/version used;
- dependency publication/pinning disposition;
- install-unit artifact matrix;
- service ownership matrix;
- drain/update/rollback evidence;
- helper compatibility evidence;
- actual hosted manager matrix;
- confirmation that producer packaging remains outside this milestone;
- residual findings and next Operations M003 blocker state.
