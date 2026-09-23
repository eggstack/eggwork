# Security, Isolation, and Resource Enforcement Roadmap

Status: closed for the qualified Linux systemd user-manager path; other platform backends remain unsupported

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

### M004 — Security closure and cross-platform adversarial qualification

Class: invariant/polish

Status: blocked on Foundation M003, Control Plane M003, and Operations M002 closure

Objective:

Run hostile input/process/workspace/transport fixtures, audit dependencies and unsafe code, validate privilege/service configurations, and close high/medium findings.

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
