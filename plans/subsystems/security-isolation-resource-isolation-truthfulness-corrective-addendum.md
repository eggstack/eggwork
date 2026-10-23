# Security, Isolation, and Resource Enforcement Post-Closure Corrective — Isolation and Authorization Truthfulness

Status: active corrective; M005a proposed, M005b proposed, M005c blocked

Predecessor roadmap:

- `plans/subsystems/security-isolation-resource-roadmap.md`

Historical predecessor closures:

- `plans/closure/security-isolation-resource/002-status.md` — Landlock runtime and `/dev/null` handling
- `plans/closure/security-isolation-resource/003-status.md` — enforced resource controls
- `plans/closure/security-isolation-resource/004-status.md` — security closure and cross-platform adversarial qualification
- `plans/closure/security-isolation-resource-remote-admission-corrective/001-status.md` — the remote-admission and capability-truthfulness corrective (C001)
Canonical authority remains:

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`
- `plans/003-planning-process.md#verification-philosophy`

Finding source:

- repository-wide code interrogation pass over the reviewed baseline.
- **SI-1 and SI-2 are new.** Neither is recorded in the `architecture/overview.md` divergences table.
- **SI-3 is not new.** The inert resource-scoped authorization hook is already recorded as divergence #4 in the `architecture/overview.md` divergences table and as an open remaining boundary in `plans/security/threat-model.md`. M005c is a *decision about an already-recorded gap*, not a new defect report, and this addendum must not present it as one.

## 1. Findings

This subsystem owns the "isolation claims are earned" invariant, the single owner of OS process lifecycle (`eggwork-runner`), and the standalone Landlock helper beneath it. The two findings below are about **truthfulness of what the node reports**: a caller must be able to tell, from the protocol it consumes, whether the isolation it asked for was actually applied, and whether the boundary it was told exists is the boundary that is enforced.

### 1.1 SI-1 — A degraded isolation is recorded but never streamed

`IsolationRequirement::BestEffort` legitimately permits degradation, and the degradation *is* recorded. `crates/eggwork-core/src/lib.rs` defines `SandboxResult::NotApplied { reason }` and `ResourceDimensionResult::NotApplied { reason }`. `crates/eggwork-runner/src/lib.rs` produces both: `TrustedLandlockSetup::prepare` returns `SandboxOutcome::NotApplied { reason }` for an unsupported sandbox profile and for an untrusted helper, `ResourceSetupOutcome::NotApplied { reason }` when the cgroup probe fails or the helper cannot be trusted, and `RunnerResult::execution_result` converts them into the core types. The server carries them into the terminal snapshot.

But the **event stream carries nothing**. `crates/eggwork-core/src/lib.rs` defines `ExecutionEventKind::Diagnostic(String)` as the only non-output, non-state event kind. At the reviewed baseline it is constructed in exactly three places across the workspace, and all three are test code:

- `crates/eggwork-server/src/lib.rs` — inside the `#[cfg(test)] mod tests` block (two constructions).
- `crates/eggwork-server/src/store.rs` — inside that file's `#[cfg(test)] mod tests` block (one construction, a payload-length probe).

There is **no production diagnostic event in the repository**.

The runner degrades in several distinct ways, and the most consequential one is a helper handshake failure. In `crates/eggwork-runner/src/lib.rs`, `LocalProcessRunner::run` performs the trusted Landlock setup, builds a `SandboxChannel`, spawns the helper as the tree leader, and waits on the helper's handshake. When the handshake reports that the helper could not enforce the requested profile (`Ok(None)`), or fails before the target (`Err(SandboxWaitError::BeforeTarget(..))`), the runner — for a best-effort request only — kills the helper tree, rewrites `setup.sandbox` and `setup.resources` to `NotApplied`, and **re-spawns the target directly, bypassing the helper entirely**, through `direct_command`. The target then runs with no Landlock ruleset at all.

The consequence is a truthfulness gap, not a cosmetic one. A caller that streams events sees `State(Running)`, the target's stdout and stderr, and `State(Succeeded)`. Nothing in that stream says the execution was not confined. The degradation is discoverable only by polling the terminal snapshot, where `ExecutionResult.sandbox` reads `NotApplied { reason }` — a field a streaming client has no reason to fetch. For a protocol whose purpose is to deliver machine-readable lifecycle facts, "the guarantee you asked for was not applied" is a lifecycle fact, and it is missing from the lifecycle channel.

Two further facts sharpen this:

- `crates/eggwork-server/src/lib.rs`'s `publish` returns early when the recorded base state is already terminal, and `store::ExecutionStore::commit_event` refuses to append to a terminal execution. A diagnostic published *after* the terminal event would be **silently dropped**, not merely late. The ordering constraint is therefore load-bearing, not stylistic.
- The `Required` path is a different case and is already truthful in the stream. A required request that cannot be satisfied makes the runner return an error before the target starts, and the server's `failed_result` synthesizes `SandboxResult::Failed { reason: "required sandbox setup failed" }` into a terminal `State(Failed)` event. The gap is specifically the *successful* execution whose isolation was degraded: there the terminal event says "succeeded" and nothing else in the stream qualifies it.

### 1.2 SI-2 — The sandbox environment deny-list is maintained twice, with different semantics

`crates/eggwork-runner/src/lib.rs` defines `fn denied_environment_key(key: &str) -> bool`. `crates/eggwork-sandbox-helper/src/main.rs` defines its own `fn denied_environment_key(key: &str) -> bool` inside `mod linux`. At the reviewed baseline the two lists are **element-for-element identical**: the same 21 entries in the same order — `LD_`, `DYLD_`, `PATH`, `IFS`, `SHELLOPTS`, `CDPATH`, `BASH_ENV`, `ENV`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS`, `RUBYOPT`, `PERL5OPT`, `GIT_ASKPASS`, `SSH_ASKPASS`, `GIT_CONFIG`, `GIT_CONFIG_`, `AWS_`, `AZURE_`, `GOOGLE_`, `SSH_AUTH_SOCK` — matched by the same rule, `upper == denied || upper.starts_with(denied)` on an ASCII-uppercased key. Two observations about the rule itself: `GIT_CONFIG_` is redundant because the `GIT_CONFIG` prefix already covers `GIT_CONFIG_GLOBAL` and friends, and because the match is prefix-based, `PATH` denies *any* key beginning with `PATH`, not only `PATH` itself.

The **semantics differ**, and they differ in the dangerous direction:

- The runner **silently drops** denied keys. `direct_command` calls `command.env_clear()`, installs `baseline_child_environment`, and then installs each caller-supplied environment entry for which `denied_environment_key` is false. `SandboxChannel::new` does the same for the helper's launch specification. A denied key is simply never passed to the child.
- The helper **fails the whole launch**. In `read_spec`, `crates/eggwork-sandbox-helper/src/main.rs` rejects a specification whose environment contains a denied key (`denied_environment_key(key) && key != "PATH"`) or whose `PATH` is not exactly `/usr/bin:/bin` (`key == "PATH" && value != "/usr/bin:/bin"`), returning the distinct status code 12. The runner reads that status as a setup failure before the target starts.

The helper also enforces a rule with **no runner counterpart**: `PATH` must equal exactly `/usr/bin:/bin`. The runner satisfies that only incidentally, through the platform branch of `baseline_child_environment`, which sets `PATH` to `/usr/bin:/bin` on non-Windows and to `%SystemRoot%\System32;%SystemRoot%` on Windows. The helper is a Linux-only binary (`#[cfg(target_os = "linux")] mod linux`), so the Windows shape never reaches it — but the rule is a property of the *helper's* environment contract, and it is expressed in only one of the two places that own the environment.

Because the helper's copy is the enforcement point for every Landlock run, the two failure directions are asymmetric:

- An entry added **only on the runner side** is silently unenforced by the helper. The runner drops the key from the spec it builds; the helper never sees it and never refuses. The security control does not exist.
- An entry added **only on the helper side** turns a valid launch into an opaque exit-12 failure. The runner does not drop the key, so the key reaches the helper, and the helper refuses the whole launch with a status code whose meaning is not visible at the call site.

The duplication is **architecturally forced**, not accidental carelessness. `crates/eggwork-sandbox-helper/Cargo.toml` has **no workspace-crate dependency** on any production crate — only `serde`, `serde_json`, and the target-scoped `landlock` and `nix` — and `architecture/sandbox-helper.md` states the rule: the helper is a standalone binary whose shipped dependency surface is deliberately kept to serde/serde_json plus one sandbox crate, because it is the trusted artifact that is staged, verified, and run with its own trust rules. `AGENTS.md`'s hard invariant 3 ("core/helper → nothing") is the same rule in one line. The fix therefore **cannot** be "share a crate": the correct shape is a shared *artifact* — a generated list, or a checked-in source of truth that both sides read — plus a test that pins the two sides together so a one-sided edit fails the build.

Two related duplications sit in the same neighbourhood and are in scope for the same milestone because they are the same class of defect:

- The **baseline child environment is written twice inside the runner**. `baseline_child_environment` produces the direct child's environment; `SandboxChannel::new` writes a second, separate literal list for the helper's launch specification. The two lists carry the same eight keys and the same values on Linux, but they are independent literals that nothing keeps in step.
- The **protocol limit constants are duplicated from `eggwork-core` into the helper**. `crates/eggwork-sandbox-helper/src/main.rs` re-declares `MAX_ARG_COUNT`, `MAX_ARG_BYTES`, `MAX_ENV_COUNT`, `MAX_ENV_NAME_BYTES`, and `MAX_ENV_VALUE_BYTES` with the same values that `crates/eggwork-core/src/lib.rs` exports and that `crates/eggwork-runner/src/lib.rs` imports. (`MAX_SPEC_BYTES` is helper-only and has no core counterpart.)

The repository already holds the pattern that answers this. `crates/eggwork-runner/tests/windows_process_tree.rs` keeps a hand-maintained `BASELINE_KEYS` list beside the runner's own `baseline_child_environment`, with the comment *"Kept in sync with `baseline_child_environment` in the crate; a mismatch is a contract change, not a test fix."* That is a comment, not a gate. M005b generalizes the pattern from a comment to a test that fails.

### 1.3 SI-3 — Resource-scoped authorization is inert on every production path

This finding is **already known**. `architecture/overview.md` records it as divergence #4: *"`GrantAuthorizer` implements only `Authorizer::authorize`, **not** the resource-aware `authorize_request` hook. Resource-scoped authorization is therefore inert on the real deployment path; ownership is enforced only in the store, by `principal_id` comparison."* `plans/security/threat-model.md` states the same boundary in the "Authenticated unauthorized principal" row: *"Broad BlobRead capability permits any known digest unless resource-scoped policy is configured."*

The machinery is fully built and publicly exported:

- `crates/eggwork-server/src/lib.rs` defines `pub enum ResourceId { Execution, Blob, Workspace, Artifact }` — a typed resource identifier.
- The same file defines `pub struct OperationRequest { operation, resource: Option<ResourceId> }`.
- The same file defines `fn resource_for_path(operation, path) -> Option<ResourceId>`, which maps a request path segment to a typed resource for `Observe`, `Cancel`, `Renew`, `Events`, `BlobRead`, `BlobWrite`, and `ArtifactRead`.
- The `Authorizer` trait in the same file declares `fn authorize_request(&self, principal, request: &OperationRequest) -> bool`, whose default implementation delegates to `fn authorize(&self, principal, operation) -> bool`.
- `crates/eggwork-server/src/operations.rs` defines `pub struct GrantAuthorizer(HashMap<PrincipalId, HashSet<Operation>>)` and implements **only** `authorize`. The trait's default `authorize_request` therefore applies, and the typed `ResourceId` is **never consulted** on the production path.
- `fn authorize_resource(state, principal, operation, resource)` in `crates/eggwork-server/src/lib.rs` is the resource-scoped call helper. It is called at six handler sites: the blob-find-missing handler (`Operation::BlobRead`, `ResourceId::Blob`), the blob-prepare handler (`Operation::BlobWrite`, `ResourceId::Blob`), the workspace-create handler (`Operation::WorkspaceCreate`, `ResourceId::Workspace`), the workspace-derive handler (`Operation::WorkspaceCreate`, `ResourceId::Workspace`), and the execute handler (twice — `Operation::Execute` with `ResourceId::Execution`, and `Operation::Execute` with `ResourceId::Workspace` when the request names one). The `dispatch` function additionally calls `authorize_request` with a `resource_for_path`-derived resource for every routed request.

Every one of those call sites resolves to the same operation-level decision. The typed resource is constructed, passed, and discarded.

The practical effect is bounded but real, and it is exactly the boundary the threat model already names: a principal holding `BlobRead` may read **any** digest the node knows, not only the ones it was granted. The store's `principal_id` fence does not help for blobs, because blob storage is content-addressed and shared; the fence that does help — `ExecutionStore::load_snapshot_for_principal`, the artifact store's `principal_id`-scoped `load` and `list`, and the workspace store's `(principal_id, base_manifest_digest)` scoping — is a *store* fence, not an *authorizer* fence, and it is applied after the resource has already been resolved and authorized at the operation level.

The only exerciser of the resource-scoped path is a unit test in `crates/eggwork-server/src/lib.rs`'s test module, which asserts that `resource_for_path` maps a path to a `ResourceId`. No production authorization decision depends on the answer.

**This is a decision, not a bug fix.** Two dispositions are coherent, and they are not equivalent in cost or in what they promise:

- **Implement resource-scoped grants.** Make `GrantAuthorizer` implement `authorize_request` against a grant shape that names resources, with an owner fence for the resource kinds that have one.
- **Remove the hook and its plumbing.** Delete `ResourceId`, `OperationRequest.resource`, `resource_for_path`, the six resource-scoped call sites, and the unit test, so the mechanism stops looking available. This is a public-API removal: `ResourceId`, `OperationRequest`, and the `Authorizer` trait are `pub` in `eggwork-server`, which is a published workspace crate.

A third disposition — deferring while advertising a "not available" marker — is weaker than it looks, because the mechanism would still be present and still inert.

Per `plans/003-planning-process.md` §8, the choice between these is a durable architecture decision and requires an ADR. M005c is therefore written now, to preserve design intent, and is **blocked** on that ADR: `plans/adrs/ADR-0007-resource-scoped-authorization.md` (**proposed, not accepted**).

## 2. Corrective milestones

### M005a — Degraded isolation observability in the event stream

Class: invariant

Status: proposed

Implementation plan:

- `plans/implementation/security-isolation-resource/005a-degraded-isolation-observability-corrective.md`

Objective:

Make a degraded isolation or resource enforcement **visible in the event stream**, not only in the terminal snapshot, and make the terminal state non-misleading for a caller that never polls a snapshot. The diagnostic must reach the caller before the target's output and before the terminal event. The `Required` path must not change: a required request still fails closed before the target starts.

### M005b — Sandbox-helper environment parity

Class: invariant

Status: proposed

Implementation plan:

- `plans/implementation/security-isolation-resource/005b-sandbox-helper-environment-parity-corrective.md`

Objective:

Give the environment deny-list, the `PATH` rule, and the child-environment baseline exactly one owner that both the runner and the standalone helper consume, without giving the helper a workspace-crate dependency, and make a one-sided edit fail the build instead of silently changing enforcement.

### M005c — Resource-scoped authorization disposition

Class: invariant

Status: blocked

Concrete blocker:

- `plans/adrs/ADR-0007-resource-scoped-authorization.md` — **proposed, not yet accepted**. The ADR must decide whether resource-scoped grants are implemented or the inert hook and its `ResourceId` plumbing are removed. Until that ADR is accepted, M005c cannot be handed off as ready, per `plans/003-planning-process.md` §4.

Implementation plan:

- `plans/implementation/security-isolation-resource/005c-resource-scoped-authorization-corrective.md`

Objective:

Resolve the already-recorded divergence #4 to a decided disposition, so that the node stops presenting a mechanism as available when it is inert — whether by implementing it or by removing it.

## 3. Validation contract after M005

M005a, M005b, and M005c freeze the following dispositions.

### 3.1 A degradation is a lifecycle fact

A best-effort isolation or resource request that could not be enforced must produce an observable event in the execution's event stream, carrying the class of degradation and a bounded reason. The event must precede the terminal event, and it must precede any output the target produces. A terminal state that reports success must never be the only signal that a requested guarantee was not applied.

### 3.2 The `Required` path is unchanged

A `Required` isolation or resource request that cannot be satisfied must continue to fail closed **before the target process starts**, and must continue to produce a terminal failure. A diagnostic is not a substitute for a refusal, and this corrective must not convert a required refusal into a best-effort degradation. This is the `AGENTS.md` hard invariant "Isolation claims are earned" and the repository's fail-closed rule, and it is the authority both M005a and M005b must preserve.

### 3.3 One owner per environment rule

The denied-key list, the `PATH` rule, and the child-environment baseline each have exactly one owner. The helper consumes the owner's output without acquiring a workspace-crate dependency. Where the runner and the helper legitimately need different *behaviour* on the same rule, that difference is stated as a deliberate contract, not left as an accident of two implementations.

### 3.4 A one-sided edit fails the build

The relationship between the runner's environment rules and the helper's environment rules is pinned by a test that compares them element-wise. A change to one side without the other is a build failure, and the failure message states that a mismatch is a contract change rather than a test to be adjusted.

### 3.5 The node does not advertise what it does not enforce

A mechanism that is present but inert must not be represented as available. This holds in both branches of the M005c decision: if resource-scoped grants are implemented, the hook decides; if the hook is removed, the public API no longer implies that resource-scoped authorization exists. In either case the `architecture/overview.md` divergences table must reflect the decided disposition rather than an open divergence.

## 4. Durable invariants

These extend the subsystem's existing invariants, including "Isolation claims are earned" and the fail-closed rule from `AGENTS.md`:

1. A degradation of isolation or resource enforcement is observable in the event stream, not only in the terminal snapshot.
2. A degraded best-effort execution's terminal state is never the only signal of the degradation.
3. The diagnostic precedes the terminal event and precedes any target output.
4. A `Required` isolation or resource request fails closed before the target process starts, and this corrective does not change that.
5. The sandbox environment deny-list, the `PATH` rule, and the child-environment baseline each have exactly one owner.
6. The sandbox helper gains no workspace-crate dependency.
7. The runner's and the helper's environment rules are pinned to each other by a test, so a one-sided edit fails the build.
8. A rule enforced by the helper is not silently unenforced by the runner, and a rule enforced by the runner alone cannot turn a valid launch into an opaque refusal.
9. The node does not present a mechanism as available when it is inert.
10. The historical M001–M004 and C001 closure records remain immutable; this corrective owns the newly discovered truthfulness defects and the escalation of the already-recorded divergence #4.
11. No change to the wire representation of `ExecutionEventKind`, `ExecutionResult`, `SandboxResult`, or `ResourceResult` under M005a or M005b.
12. The one-way crate dependency rule (core/helper → nothing; client → core; server → core + runner) is unchanged.

## 5. Compatibility and migration

**M005a** changes what a caller observes, not what it may do. A caller that today ignores `ExecutionEventKind::Diagnostic` keeps working. A caller that asserts "the only events are State/Stdout/Stderr" must be updated — that is the intended correction, and it is a client-side behaviour change that the plan must call out. The runner's public `OutputChunk` type may need a diagnostic variant to deliver the ordering guarantee; that is a release-visible change to a published crate and must be handled as a version-visible change, not a silent one.

**M005b** changes the runner's and the helper's environment contract. The deny-list's *content* need not change, but its single ownership does, and the `PATH` rule must become an explicitly stated contract on both sides (or be removed in favour of an explicit statement of the actual baseline). A caller whose environment currently includes a key that only one side denies will observe a different outcome after M005b; the closure record must enumerate the before/after disposition of every entry.

**M005c** is the compatibility-sensitive milestone. Implementing resource-scoped grants is additive to the configuration surface but changes authorization decisions. Removing the hook removes public items from `eggwork-server` (`ResourceId`, `OperationRequest`, and the `authorize_request` method on the public `Authorizer` trait), which is a breaking change to a published crate and must be reflected in the release version and notes. The ADR decides which; the plan must not pre-empt it.

## 6. Downstream relationship

- `eggwork-core` owns the event kinds and the result types. M005a and M005b consume them; neither changes their wire representation.
- `eggwork-client` is affected by M005a only as a consumer that may now receive a diagnostic event. `ExecutionStream::into_events` in `crates/eggwork-client/src/lib.rs` already yields any `ExecutionEvent`, including a diagnostic, so no client change is required to *receive* one; a client that wants a typed convenience accessor may add one.
- `eggwork-sandbox-helper` is the enforcement point for SI-2. Its standalone-binary rule is the binding constraint on M005b's mechanism, not an obstacle to be worked around.
- `eggwork-server` owns the event publication path (SI-1) and the authorization machinery (SI-3).
- The control-plane shared-contract corrective (`API_SCHEMA_VERSION` and the shared wire contract) is a sibling in the same repository-interrogation batch. M005a must not change the wire shape of `ExecutionEventKind`, so the two milestones do not collide.
- The foundation runner-request-validation corrective (`M005` in the foundation subsystem) is independent. Both touch `crates/eggwork-runner/src/lib.rs`, but in different regions: that one changes request validation, this one changes environment-rule ownership.
- Operations M007 (Windows service host) is independent and may run in parallel with all three milestones.
- M005a, M005b, and M005c are mutually independent of one another.

## 7. Exit conditions

The Security M005 batch closes only when:

- M005a, M005b, and M005c each have their own closure record under `plans/closure/security-isolation-resource/` naming the exact verification that was run;
- M005c's closure record names the accepted ADR and the decided disposition;
- a degraded best-effort execution emits a diagnostic event that precedes the terminal event and any target output, proven by a non-vacuous test;
- a `Required` request still fails closed before the target starts, proven by a non-vacuous test;
- the runner's and the helper's environment rules are pinned to each other by a test that fails on a one-sided edit;
- the helper still has no workspace-crate production dependency;
- `architecture/overview.md`'s divergences table reflects the decided disposition of divergence #4, and no new row is added for a resolved item;
- `architecture/sandbox-helper.md`, `architecture/runner-execution.md`, and `architecture/server-node.md` describe the code as it now stands;
- the full pre-submit gate is green on the reviewed head;
- no high or medium finding remains without an explicit owner.
