# Security M004 — Adversarial and Cross-Platform Closure Qualification

Status: blocked on Operations M002 closure

Source roadmap:

- `plans/subsystems/security-isolation-resource-roadmap.md`

Canonical authority:

- `plans/000-long-term-specification.md#13-security-model`
- `plans/000-long-term-specification.md#14-isolation-and-resource-enforcement`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

Closed predecessors:

- Security M001 authorization/redaction/threat model;
- Security M002 trusted Landlock sandbox;
- Security M002a `/dev/null` correction;
- Security M003 resource enforcement;
- Security remote-admission corrective C001;
- Foundation execution-ownership CI corrective C001;
- Control Plane M003.

Remaining hard dependency:

- Operations M002 Eggup deployment/service integration, because final helper/install ownership and update/restart behavior must be included in the adversarial evidence surface.

Primary class: invariant/polish

## 1. Objective

Run the final adversarial security qualification over Eggwork's actual remotely reachable execution, workspace, isolation/resource, lease, and installed-service surfaces.

This milestone primarily proves and reconciles existing controls. It may make bounded hardening fixes discovered by the fixtures, but it must not introduce a new sandbox, network-isolation backend, scheduler, or deployment owner.

## 2. Scope

The qualification covers:

- mTLS transport-derived identity;
- operation/resource authorization;
- execution idempotency/generation/lease fencing;
- workspace/blob/artifact path and ownership isolation;
- required/best-effort Landlock behavior;
- required/best-effort resource controls;
- unsupported network-restriction fail-closed behavior;
- sandbox-helper trust/version/install ownership after Operations M002;
- drain/update/restart interaction with active/retained executions;
- secret-safe errors/debug/operator output;
- crash/restart/reconciliation boundaries.

## 3. Platform truthfulness

Qualified behavior must be separated by platform/backend.

### Linux

Where hosted capabilities exist:

- trusted Landlock `workspace_rw` required isolation;
- systemd/cgroup-v2 memory/CPU/PID controls;
- installed sibling helper trust and version compatibility;
- required control failure before target spawn;
- child/descendant confinement and cleanup.

### macOS

Do not claim Landlock/cgroup enforcement.

Hosted evidence must prove:

- unsupported required isolation/resource requests are not advertised and fail closed;
- best-effort outcomes are truthful;
- mTLS/auth/workspace/lease behavior remains functional;
- service ownership/update behavior matches Operations M002's qualified launchd scope.

### Windows

Do not claim Job Objects or another hard resource backend unless one actually exists by implementation time.

Hosted evidence must prove:

- capability advertisement matches reality;
- unsupported required controls fail closed;
- path/case/device-name validation remains portable;
- mTLS/auth/workspace/lease behavior remains functional;
- service ownership/update behavior matches Operations M002's qualified SCM scope.

Compilation alone is not runtime qualification.

## 4. Adversarial fixture matrix

### Identity and authorization

Prove:

- payload principal forgery rejected;
- wrong mTLS principal cannot observe/cancel/renew/read artifacts belonging to another principal;
- resource-aware authorization applies to execution/workspace/blob/artifact operations;
- certificate-chain/pathological-input bounds hold;
- anonymous/non-client-authenticated requests have zero side effect.

### Lease/generation/idempotency

Prove:

- wrong lease token -> `invalid_lease`;
- wrong principal -> `forbidden` without leaking lease validity;
- stale generation cannot control newer generation;
- same id/generation + same digest is idempotent;
- same id/generation + different digest conflicts;
- reconnect/retry creates no second child;
- lease expiry terminates the process tree and produces truthful terminal evidence.

### Hostile workspace/input

Exercise:

- traversal;
- absolute paths;
- backslash/Windows-device paths;
- case-fold collisions;
- missing parents;
- symlink/special-file attempts;
- oversized path/count/logical bytes;
- corrupt/truncated blob;
- manifest digest mismatch;
- derived-manifest patch attacks if Workspace M004 has closed by implementation time.

Workspace M004 is not a hard dependency. If it remains unimplemented, record it as not applicable rather than fabricating coverage.

### Executed-process sandbox attacks

On qualified Linux:

- read outside workspace;
- write outside workspace;
- child process inherits confinement;
- attempts involving `/tmp`, home, repository parent, and representative sensitive runtime paths;
- required helper/setup failure produces no unsandboxed target;
- helper trust failure is fail-closed for required isolation.

Do not broaden allowed paths merely to make fixtures pass.

### Resource attacks

On qualified Linux:

- PID pressure;
- memory pressure;
- CPU control evidence;
- required unavailable controller rejects before spawn;
- best-effort unavailable control reports not-applied;
- resource-limit terminal failure remains distinguishable from ordinary exit/timeout/cancel.

### Network policy

- `Unrestricted` advertised explicitly;
- `Disabled` and `AllowListed` remain `capability_mismatch` unless a separately qualified backend exists;
- no test may imply Landlock is a network sandbox.

### Secret/redaction

Inject recognizable sentinel values into:

- client-key/cert paths;
- environment;
- request errors;
- helper failures;
- service/update candidate metadata;
- operator JSON/text diagnostics.

Assert sentinels do not appear outside explicitly authorized payload/content channels.

## 5. Deployment/update adversarial qualification

After Operations M002 closes, exercise the installed unit rather than only development-tree binaries.

Required cases:

- same-name foreign service registration cannot be stopped/replaced/uninstalled;
- helper executable/path/version mismatch blocks required isolation;
- update enters persistent drain before service replacement;
- new execution admission stops while draining;
- retained terminal records survive update/restart truthfully;
- bounded post-start health failure triggers Eggup rollback while backups are retained;
- rollback restores a coherent daemon/helper pair;
- successful update clears/reconciles drain according to documented policy;
- failed update cannot fabricate execution completion.

Filesystem rollback remains Eggup-owned.

## 6. Race and TOCTOU review

Add deterministic race fixtures where practical:

- workspace/materialization versus GC;
- terminalization versus artifact/blob GC;
- cancellation versus spawn/setup handshake;
- lease expiry versus explicit cancel;
- drain versus execute admission;
- helper replacement/trust check around launch;
- update/restart versus retained execution recovery.

Any race that cannot be made deterministic must be documented with the exact invariant and bounded stress evidence; do not claim exhaustive proof.

## 7. Static ownership and dependency audit

Re-run and inspect:

- process-spawn ownership;
- service-manager ownership after Operations M002;
- transport dependency direction;
- direct shell/service-manager subprocess usage;
- unsafe-code policy;
- secret-bearing Debug/Serialize surfaces.

No new direct `systemctl`/`launchctl`/`sc.exe`/PowerShell/crontab implementation may appear in Eggwork.

## 8. Documentation reconciliation

Update:

- threat model;
- architecture/security and operations docs;
- capability/platform matrix;
- operator guidance;
- roadmap and registry.

Documentation must distinguish:

- implemented;
- runtime-qualified;
- unsupported;
- deferred.

## 9. Required verification

Common:

```bash
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
git diff --check
```

Hosted runtime evidence:

- Linux for Landlock/cgroups/service/update;
- macOS for launchd plus explicit unsupported enforcement behavior;
- Windows for SCM plus explicit unsupported enforcement behavior.

Use exact hosted-run identifiers in closure.

## 10. Acceptance criteria

1. Every remotely reachable operation has adversarial authn/authz evidence.
2. Lease/generation/idempotency fencing survives negative and restart cases.
3. Workspace/blob/artifact confinement survives hostile inputs and GC races.
4. Required Linux Landlock confinement is physically demonstrated against outside reads/writes and descendants.
5. Required qualified resource controls are demonstrated; unsupported platforms fail closed.
6. Network restriction remains explicitly unsupported unless separately implemented.
7. Installed helper/service/update ownership is adversarially qualified after Operations M002.
8. Post-update health failure rolls back through Eggup without Eggwork-owned filesystem transaction logic.
9. Secret sentinels do not leak through ordinary diagnostics/errors/debug.
10. Hosted platform claims match the actual evidence matrix.
11. No unresolved critical/high finding remains.
12. Any medium finding has an explicit corrective owner; M004 cannot close by relabeling a material control defect as future polish.

## 11. Stop conditions

Stop and create a corrective plan/ADR if:

- a required control silently downgrades;
- service/update safety requires bypassing Eggup ownership;
- a cross-principal workspace/blob/artifact leak is found;
- a helper trust race requires redesigning the helper channel;
- a platform claim lacks runtime evidence;
- a new network sandbox or process owner is required;
- remediation changes a durable ownership/identity invariant.

## 12. Closure evidence

Create:

- `plans/closure/security-isolation-resource/004-status.md`

Record:

- exact Eggwork and Eggup revisions;
- Operations M002 closure dependency;
- adversarial requirement matrix;
- hosted Linux/macOS/Windows runs;
- physical confinement/resource evidence;
- auth/lease/workspace/GC race evidence;
- installed helper/update rollback evidence;
- secret-negative evidence;
- unresolved findings by severity;
- final Security workstream disposition.
