# Security M002 — Trusted Landlock Sandbox Path

Status: active

Dependency evidence: `plans/closure/foundation-execution-core/002-status.md` and `plans/closure/workspace-artifact-transport/002-status.md`

Source roadmap:

- `plans/subsystems/security-isolation-resource-roadmap.md`

Research input:

- current CodeGG sandbox/helper implementation and its trust-channel corrective history.

## 1. Objective

Generalize the proven child-process-only Landlock helper pattern into Eggwork for supported Linux hosts, rooted in the materialized execution workspace and explicit runtime dependencies.

## 2. Required trust properties

- helper resolved from installation-owned sibling location or another equally strong configured owner;
- never selected through target cwd/PATH/environment;
- sandbox specification stored/transmitted through owner-private bounded channel;
- helper status returned through a dedicated bounded channel;
- setup status cannot be forged by ordinary child stdout/stderr;
- required setup failure stops target execution;
- no fallback to unsandboxed process when Required.

## 3. Policy model

Define at least:

- Disabled;
- BestAvailable;
- Required(profile).

Initial profile should be narrow, such as workspace-read/write with explicitly required runtime reads and optional network policy kept separate.

Do not claim network isolation from Landlock filesystem enforcement.

## 4. Workspace integration

Allowed read/write roots derive from the prepared Eggwork workspace and explicit minimal runtime needs, not arbitrary request absolute paths.

Outputs remain confined to workspace/artifact rules.

## 5. Tests

- capability probe;
- allowed read/write;
- denied outside read/write;
- helper missing/wrong/symlinked;
- spec tamper;
- status-channel failure;
- required unsupported kernel;
- BestAvailable explicit outcome;
- cancel/timeout with sandbox;
- malicious cwd/PATH/env;
- workspace symlink attempts.

## 6. Acceptance criteria

1. Required Landlock is either verified enforced or execution is rejected/failed before target work.
2. Helper identity is installation-owned.
3. Sandbox outcome appears in machine-readable result.
4. Sandbox cannot widen workspace authority.
5. Process cleanup remains correct.

## 7. Stop conditions

Stop if the implementation needs to infer success from stderr text, discover helper through PATH, or silently run unsandboxed.

## 8. Closure evidence

Create `plans/closure/security-isolation-resource/002-status.md` with actual supported-Linux runtime evidence, not only compilation.
