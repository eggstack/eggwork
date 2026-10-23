# Security, Isolation, and Resource Enforcement Roadmap

Status: closed through M004 (remote-admission C001 and Operations M002 included); M005a and M005b implemented (closure records written, implementation commits pending); M005c blocked on ADR-0007

Canonical authority:

- `plans/000-long-term-specification.md#13-security-model`
- `plans/000-long-term-specification.md#14-isolation-and-resource-enforcement`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

## 1. Ownership boundary

This subsystem owns cross-cutting security policy and platform enforcement:

- authorization policy/matrix after transport identity;
- secret redaction;
- request/path negative validation;
- isolation requirement/result types and policy;
- trusted sandbox helper contract;
- OS resource-control backends;
- security-focused adversarial fixtures;
- audit-safe provenance fields.

Transport mechanics remain in control-plane protocol; process lifecycle remains in runner.

## 2. Durable invariants

1. Network command execution is authenticated and authorized before side effects.
2. Request bodies cannot self-grant identity/authority.
3. Required isolation fails closed.
4. Required resource enforcement fails closed.
5. Helper selection cannot be redirected by hostile cwd/PATH/environment.
6. Secrets do not enter ordinary logs/errors/events/debug.
7. Resource exceedance and process exit remain distinguishable.
8. Unsupported platform capability is explicit.

## 3. Milestones

### M001 — Authorization/redaction/threat-model foundation

Class: invariant/infrastructure

Status: closed

Implementation plan:

- `plans/implementation/security-isolation-resource/001-authorization-redaction-and-threat-model.md`

Objective:

Define principal-to-capability policy, operation descriptors, secret-safe serialization/debugging, threat model, and adversarial request corpus.

Exit conditions:

- authorization matrix covers every network operation;
- deny creates zero side effect;
- principal fields cannot be accepted from payload;
- secret-negative tests cover routes, env, cert metadata, and errors;
- threat model documents malicious controller, malicious executed process, and hostile workspace inputs.

### M002 — Trusted filesystem sandbox path

Class: invariant/capability

Status: closed

Closure evidence: `plans/closure/security-isolation-resource/002-status.md`

Implementation plan:

- `plans/implementation/security-isolation-resource/002-trusted-landlock-sandbox-path.md`

Objective:

Generalize the trusted CodeGG Landlock/helper design into Eggwork's runner for supported Linux hosts.

Exit conditions:

- helper resolved from installation-owned location;
- bounded spec/status channel;
- required setup failure stops execution;
- allowed roots exactly reflect materialized workspace/runtime requirements;
- sandbox outcome is typed terminal evidence.

### M003 — Enforced resource controls

Class: capability/invariant

Status: closed for the qualified Linux systemd user-manager path

Closure evidence: `plans/closure/security-isolation-resource/003-status.md`

Host note: direct writes to this session cgroup are unavailable; the systemd user manager can create transient scopes. The backend probes each requested controller and verifies the cgroup properties before target start.

Implementation plan:

- `plans/implementation/security-isolation-resource/003-enforced-resource-controls.md`

Objective:

Add truthful OS-backed resource enforcement with Linux cgroups v2 first, appropriate Unix rlimits, Windows Job Objects, and explicit unsupported/best-effort macOS semantics.

Exit conditions:

- required unsupported limits reject before spawn;
- memory/PID/CPU enforcement tests prove actual behavior on supported hosts;
- cleanup still reaches descendants;
- enforcement failures cannot become advisory silently;
- capability report matches runtime probes.

### C001 — Remote enforcement admission and capability truthfulness

Class: invariant/corrective

Status: closed

Closure evidence: `plans/closure/security-isolation-resource-remote-admission-corrective/001-status.md`

Corrective authority:

- `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md`

Implementation plan:

- `plans/implementation/security-isolation-resource-remote-admission-corrective/001-remote-enforcement-admission-and-capability-truthfulness.md`

Objective:

Expose the already-implemented Landlock/resource enforcement path through authenticated remote execution, unify runtime capability advertisement across `/v1/capabilities` and status, and keep unsupported network restrictions fail-closed.

This corrective was discovered by real downstream CodeGG qualification and does not rewrite M002/M003 historical closure evidence.

### M004 — Security closure and cross-platform adversarial qualification

Class: invariant/polish

Status: closed

Closure evidence: `plans/closure/security-isolation-resource/004-status.md`
(implementation `283f3ea`)

Implementation plan:

- `plans/implementation/security-isolation-resource/004-security-closure-and-cross-platform-adversarial-qualification.md`

Closed prerequisites include remote-admission corrective C001, Foundation ownership-CI corrective C001, and Control Plane M003.

Objective:

Run hostile input/process/workspace/transport fixtures, audit dependencies and unsafe code, validate privilege/service configurations, and close high/medium findings.

### M005a — Degraded-isolation observability corrective

Class: invariant

Status: implemented — closure record written, implementation commit pending

Closure evidence: `plans/closure/security-isolation-resource/005a-status.md`
(implementation `6f41957`)

Corrective addendum:

- `plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md`

Implementation plan:

- `plans/implementation/security-isolation-resource/005a-degraded-isolation-observability-corrective.md`

Objective:

Make degradation visible where a caller will actually see it. `SandboxResult::NotApplied` is recorded truthfully in the terminal snapshot, but `ExecutionEventKind::Diagnostic` has no production publisher — it is emitted only from test code — so a caller streaming events sees an ordinary successful execution when best-effort isolation or resource enforcement was not applied. The `Required` path must not change: a required request still fails closed before the target spawns.

Exit conditions:

- a degraded best-effort execution emits a diagnostic into the event stream, with a payload that identifies what degraded and why;
- the diagnostic is published **before the target's first output and before the terminal event**, so a caller sees the degradation while the execution is still running. The runner emits it on the output channel it already owns, and the payload and the terminal snapshot are derived from the same `SetupOutcome`, so the two cannot disagree. A diagnostic derived from the runner's result instead would necessarily arrive after all target output — and after the terminal event, where the store would refuse to append it;
- no `Required` execution reaches the target without the requested control;
- the required-isolation and required-resource fail-closed rules are unchanged.

### M005b — Sandbox-helper environment parity corrective

Class: invariant

Status: implemented — closure record written, implementation commit pending

Closure evidence: `plans/closure/security-isolation-resource/005b-status.md`
(implementation `63699df`)

Corrective addendum:

- `plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md`

Implementation plan:

- `plans/implementation/security-isolation-resource/005b-sandbox-helper-environment-parity-corrective.md`

Objective:

Give the sandbox environment deny-list one source of truth. The runner and the standalone sandbox helper maintain the same list independently and apply it with different semantics — the runner silently drops denied keys, the helper rejects the whole launch — and the helper additionally enforces a `PATH` rule the runner does not. The helper's no-workspace-dependency rule makes the duplication architecturally forced, so the fix is a shared artifact plus a pinning test, not a shared crate.

The duplication is live, and this milestone closes it. The runner's launch-specification builder starts from a fixed baseline and then appends every non-denied caller key, and none of the baseline keys except `PATH` is denied. A caller that sets `LANG` therefore produces a launch specification with two `LANG` entries; the helper refuses a specification with duplicate keys; and for a best-effort request the runner answers that pre-target setup failure by re-spawning the target **with no Landlock ruleset at all**. The observable result is a successful, unconfined execution whose only trace of the degradation is a `NotApplied` in the terminal snapshot.

Exit conditions:

- one source of truth exists that the helper binary can consume without gaining a workspace-crate dependency;
- the drop-versus-reject difference is decided explicitly per side and documented;
- a caller-supplied key that collides with a node-owned baseline key does not produce a duplicate-key launch specification, and does not cause a best-effort execution to lose its confinement;
- the direct path and the helper path agree on which value wins for a reserved key;
- the `PATH` rule is either enforced on both sides or replaced by an explicit statement of the actual child-environment baseline;
- a test pins the two lists element-wise, so a one-sided edit fails the build.

### M005c — Resource-scoped authorization corrective

Class: invariant

Status: **blocked** — concrete blocker is ADR-0007, `plans/adrs/ADR-0007-resource-scoped-authorization.md` (**proposed, not accepted**)

Corrective addendum:

- `plans/subsystems/security-isolation-resource-isolation-truthfulness-corrective-addendum.md`

Implementation plan:

- `plans/implementation/security-isolation-resource/005c-resource-scoped-authorization-corrective.md`

Objective:

Decide and then close the disposition of the resource-scoped authorization hook. The production authorizer implements only the operation-level check, so the typed `ResourceId` carried by every request is never consulted, and a principal granted blob-read can read any known digest. The gap is already recorded as divergence #4, so this milestone is a decision about a known gap rather than a new defect report. Per `plans/003-planning-process.md` §4 the plan is written early to preserve design intent while the decision is pending.

Exit conditions:

- ADR-0007 is accepted and states whether resource-scoped grants are implemented or the hook and its plumbing are removed;
- the node does not present a mechanism as available while it is inert;
- the divergences-table entry is updated to the decided disposition.

## 4. Capability semantics

Requirements should encode whether a caller requests:

- not requested;
- best effort;
- required.

The node reports what was actually enforced. A required mode cannot be satisfied by an admission hint.

## 5. Verification strategy

- authz matrix/static guard;
- secret-negative serialization tests;
- sandbox escapes/path attacks;
- resource exhaustion;
- fork/process-tree pressure;
- hostile stdout/stderr volume;
- cancellation during enforcement setup;
- helper tampering/symlink/PATH attacks;
- platform-hosted tests for controls that cannot be truthfully verified through cross compilation.
