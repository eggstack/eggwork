# Security / Isolation / Resource Post-Closure Corrective — Remote Admission and Capability Truthfulness

Status: active corrective; C001 ready

Predecessor roadmap:

- `plans/subsystems/security-isolation-resource-roadmap.md`

Historical predecessor closures:

- `plans/closure/security-isolation-resource/002-status.md` — trusted Landlock workspace sandbox path
- `plans/closure/security-isolation-resource/003-status.md` — Linux systemd/cgroup resource enforcement

Canonical authority remains:

- `plans/000-long-term-specification.md#13-security-model`
- `plans/000-long-term-specification.md#14-isolation-and-resource-enforcement`
- `plans/002-long-term-roadmap.md#phase-5--isolation-and-resource-enforcement`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`

Finding source:

- downstream CodeGG live-node qualification at `d3d390d56620fb5c6f755a5dfb0987e0a1c01651`
- reviewed Eggwork head before this corrective: `faaa0b905fa6bc43e46825fdd98530b5533a970f`

## 1. Findings

Live CodeGG -> Eggwork qualification exposed a gap between Eggwork's closed runner/security capabilities and its network admission path.

### 1.1 Remote execute rejects implemented isolation requirements

`RunnerRequest::from_spec` already maps:

- `IsolationRequirement::None` -> `SandboxRequest::None`;
- `IsolationRequirement::BestEffort` -> `SandboxRequest::BestEffort { profile: "workspace_rw" }`;
- `IsolationRequirement::Required` -> `SandboxRequest::Required { profile: "workspace_rw" }`.

The trusted runner path can apply the closed Linux Landlock `workspace_rw` profile and fails closed for required setup.

However, `eggwork-server` rejects every network request whose isolation requirement is not `None` before `RunnerRequest::from_spec` is allowed to carry that policy into the runner.

This makes the closed isolation capability unavailable to remote callers.

### 1.2 Capability responses are not one truthful snapshot

At the reviewed baseline:

- `GET /v1/capabilities` returns only a static feature list;
- `GET /v1/status` returns the static features plus `state.resource_capabilities`.

A controller therefore receives different feature sets depending on which capability-bearing endpoint it calls. CodeGG preflight uses `/v1/capabilities` for required capability checks, so remotely enforceable resource facts may be hidden from the exact endpoint intended for preflight.

### 1.3 Network isolation is modeled but not implemented

`NetworkRequirement` includes:

- `Unrestricted`;
- `Disabled`;
- `AllowListed(Vec<String>)`.

No current Eggwork backend enforces disabled or allow-listed networking. The server's current rejection of those requirements is therefore correct and must remain fail-closed.

The corrective MUST NOT pretend that Landlock filesystem isolation supplies network isolation.

## 2. Corrective milestone

### C001 — Remote enforcement admission and truthful capability projection

Class: invariant/corrective

Status: ready

Implementation plan:

- `plans/implementation/security-isolation-resource-remote-admission-corrective/001-remote-enforcement-admission-and-capability-truthfulness.md`

Objective:

Make remote execution reach already-implemented runner isolation/resource enforcement when the node can actually provide it, and expose one consistent runtime-probed capability set to callers.

## 3. Capability contract

C001 freezes these feature-string semantics for the existing `NodeCapabilities.features` protocol:

- `exec.argv.v1` — existing argv execution support.
- `isolation.landlock.workspace-rw.v1` — this node has runtime-qualified support for the trusted Landlock `workspace_rw` profile used by `IsolationRequirement::{BestEffort, Required}`.
- `network.unrestricted.v1` — the node can execute with unrestricted networking. This is descriptive, not a security guarantee.
- existing `resources.cgroups-v2.memory`, `resources.cgroups-v2.cpu`, and `resources.cgroups-v2.pids` retain their current meanings.

C001 MUST NOT advertise:

- `network.disabled.v1`;
- any allow-list network feature;
- Landlock capability merely because the helper file exists;
- resource capabilities that runtime probes did not establish.

Future network-isolation work may add separately versioned feature names only when an enforcement backend exists.

## 4. Admission semantics

Remote execution admission after C001:

### Isolation

- `None`: accepted without sandbox request.
- `BestEffort`: accepted and passed to the canonical runner. If enforcement becomes unavailable between capability probe and launch, terminal evidence MUST say `NotApplied`; it must never claim applied.
- `Required`: if the node does not currently advertise the Landlock workspace capability, reject before reservation/target spawn with typed `409 capability_mismatch`. If capability was advertised but setup later races/fails, the canonical runner still fails closed before target spawn.

### Resources

- `NotRequested`: accepted.
- `BestEffort`: accepted and passed to the runner, with truthful `Applied`/`NotApplied` terminal evidence.
- `Required`: when the required dimension is not in the runtime capability snapshot, reject before reservation/target spawn with `409 capability_mismatch`. A later setup race still fails closed before target spawn.

### Network

- `Unrestricted`: accepted.
- `Disabled`: `409 capability_mismatch` until a real backend exists.
- `AllowListed(_)`: `409 capability_mismatch` until a real backend exists.

There is no silent downgrade from disabled/allow-listed to unrestricted.

## 5. Durable invariants

1. Required isolation/resource controls fail closed before target spawn.
2. Capability advertisement reflects a runtime probe, not compile target or configuration intent.
3. `/v1/capabilities` and `/v1/status` expose the same capability feature set for one node snapshot.
4. A capability disappearing after preflight cannot become silent unisolated execution for a `Required` request.
5. Best-effort results remain explicit and truthful.
6. Network restrictions remain unsupported until actually enforced.
7. One canonical runner/setup path remains authoritative.
8. No scheduler/placement policy enters Eggwork.
9. No authentication, lease, generation, workspace, or artifact semantics weaken.
10. Historical M002/M003 closure records remain immutable; this corrective owns the newly discovered remote-admission defect.

## 6. Downstream relationship

- Eggwork Operations M002 is independent and remains ready.
- Security M004 MUST wait for C001 and Operations M002 so the final adversarial pass covers the actual remotely reachable enforcement surface.
- CodeGG M002 may implement capability/posture projection in parallel against the feature contract in this addendum.
- CodeGG must not claim real restricted-spec qualification until C001 closes and CodeGG re-runs its live node fixture against the corrected Eggwork revision.

## 7. Exit conditions

C001 closes only when:

- `/v1/capabilities` and `/v1/status` agree on runtime capability features;
- required Landlock isolation is remotely admitted on a qualified Linux host and reports applied;
- an outside-workspace access attempt is denied under a remotely submitted required-isolation execution;
- required isolation rejects before target spawn when unavailable;
- best-effort unavailable isolation returns truthful not-applied evidence;
- required resource dimensions reject before target spawn when unavailable and execute when available;
- disabled/allow-listed network requests remain typed capability mismatches;
- unrestricted network support is explicitly advertised, not inferred;
- CodeGG-style real NodeClient/mTLS restricted-spec fixture can pass against the corrected node;
- no high/medium finding remains without an explicit owner.
