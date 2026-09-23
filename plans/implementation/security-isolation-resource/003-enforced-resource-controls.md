# Security M003 — Enforced Resource Controls

Status: active

Source roadmap:

- `plans/subsystems/security-isolation-resource-roadmap.md`

## 1. Objective

Implement truthful OS-backed resource enforcement and advertise only capabilities the current node can actually enforce.

## 2. Initial dimensions

Prioritize:

- process/PID count;
- memory;
- CPU quota/weight where meaningful;
- wall-clock remains runner timeout;
- file size/open file/other narrow rlimits where useful;
- IO controls only if semantics and privilege requirements are stable enough.

Keep resource admission hints distinct from enforcement.

## 3. Requirement semantics

For each dimension represent:

- not requested;
- best effort;
- required.

Node capability reports supported enforcement backends/dimensions.

If required cannot be established, reject before target spawn.

Best effort must report what was actually applied.

## 4. Platform strategy

### Linux

Prefer cgroups v2 for memory/PIDs/CPU where available and permitted. Define rootless/user-service requirements and delegation limitations explicitly.

Use rlimits only for dimensions they truthfully enforce.

### Windows

Use Job Objects for process-tree ownership and supported resource limits.

### macOS

Expose only controls proven enforceable. Do not label admission estimates or unsupported sandbox concepts as hard resource limits.

## 5. Lifecycle ordering

- allocate/configure resource container before target spawn where possible;
- attach target atomically enough to prevent escape;
- hold ownership until descendants cleaned;
- capture limit-exceeded evidence;
- remove/reclaim resource container after cleanup;
- restart handling must not blindly kill unrelated reused IDs/paths.

## 6. Tests

- required unsupported;
- memory exceed;
- PID/fork pressure;
- CPU behavior where deterministic fixture possible;
- cancel/timeout under limits;
- descendant cleanup;
- setup failure;
- service/rootless permission failure;
- capability probe drift;
- concurrent executions isolated from each other's limits.

Hosted platform tests are required for claims of actual enforcement.

## 7. Acceptance criteria

1. Hard-limit claims have direct runtime evidence.
2. Required unsupported rejects before spawn.
3. Best-effort is labeled as such in result.
4. Limit exceedance is distinguishable from normal nonzero exit.
5. Resource containers cannot leak descendants.
6. Capability/status reflects current host.

## 8. Closure evidence

Create `plans/closure/security-isolation-resource/003-status.md` with per-platform capability/enforcement matrix and actual host evidence.
