# Security Remote-Admission Corrective C001 — Remote Enforcement Admission and Capability Truthfulness

Status: ready for handoff

Source corrective:

- `plans/subsystems/security-isolation-resource-remote-admission-corrective-addendum.md`

Historical predecessor evidence:

- `plans/closure/security-isolation-resource/002-status.md`
- `plans/closure/security-isolation-resource/003-status.md`

Plan-authoring Eggwork baseline:

- `faaa0b905fa6bc43e46825fdd98530b5533a970f`

Downstream evidence that exposed the gap:

- CodeGG corrective closure `d3d390d56620fb5c6f755a5dfb0987e0a1c01651`

Primary class: invariant/corrective

## 1. Objective

Remove the server-only barrier that prevents remote callers from using Eggwork's already-implemented Landlock/resource enforcement, while making node capability advertisement one consistent runtime-probed contract.

This plan does not implement network isolation.

## 2. Current implementation evidence

At the baseline:

- `eggwork-core::CommandSpec` already carries `ResourceRequirements`, `IsolationRequirement`, and `NetworkRequirement`.
- `eggwork-runner::RunnerRequest::from_spec` already translates isolation and resources into the canonical setup path.
- `TrustedLandlockSetup` already owns trusted-helper verification, Landlock execution, and systemd/cgroup resource probes.
- `eggwork-server::execute` rejects any request not equal to `IsolationRequirement::None + NetworkRequirement::Unrestricted` before `RunnerRequest::from_spec`.
- `NodeState` stores only `resource_capabilities`.
- `GET /v1/capabilities` returns a static list and omits those dynamic resource features.
- `GET /v1/status` appends `resource_capabilities`.

The defect is therefore primarily admission/capability plumbing, not missing runner enforcement.

## 3. Work package A — One runtime capability snapshot

Replace the resource-only server snapshot with a general execution capability snapshot owned by the runner/setup layer.

Preferred shape:

- add a typed/internal runner capability report or a single `execution_capabilities() -> Vec<String>` boundary;
- let `TrustedLandlockSetup` contribute both isolation and resource facts;
- keep `NodeCapabilities.features` as the wire representation for protocol compatibility.

Required advertised features:

```text
isolation.landlock.workspace-rw.v1
network.unrestricted.v1
resources.cgroups-v2.memory
resources.cgroups-v2.cpu
resources.cgroups-v2.pids
```

plus existing static protocol/workspace/artifact features.

Landlock capability MUST require more than helper-path existence. The probe must establish:

1. trusted helper path/ownership/mode requirements;
2. supported Landlock ABI/rights sufficient for the `workspace_rw` profile;
3. no known setup prerequisite is missing at node startup.

Use a bounded non-target probe. Do not launch an arbitrary user command as a capability probe.

If the cleanest implementation is to add a bounded helper probe mode, it must:

- execute only the trusted helper;
- spawn no target process;
- have a fixed timeout;
- return a bounded machine-readable/result code;
- reuse the same profile enforcement logic used for actual launches.

If an in-process Landlock ruleset capability probe can prove the exact required rights without duplicating policy, that is also acceptable.

## 4. Work package B — Single capability construction function

Create one server-side function for the current `NodeCapabilities` value.

Both:

- `GET /v1/capabilities`; and
- `NodeStatus.capabilities`

MUST call/use the same feature set.

Do not maintain two literal static feature lists.

Required tests:

- capabilities/status feature equality;
- dynamic resource feature present in both when probed;
- Landlock feature present in both only when qualified;
- feature count/bounds still satisfy `NodeCapabilities::validate`.

## 5. Work package C — Isolation admission

Remove the blanket `isolation == None` admission requirement.

Implement:

### None

Accept and map through `RunnerRequest::from_spec` unchanged.

### BestEffort

Accept even when the Landlock capability is absent.

The canonical runner decides at setup time:

- `Applied { profile: "workspace_rw" }` when enforcement succeeds;
- `NotApplied { reason }` when unavailable.

The server MUST NOT convert best-effort into required or claim applied before runner evidence exists.

### Required

Before reserving the execution identity or starting a target:

- verify the runtime capability snapshot includes `isolation.landlock.workspace-rw.v1`;
- otherwise return `409 capability_mismatch` and increment `rejected_capability`.

Still preserve the runner's fail-closed setup check. Capability advertisement is an optimization/preflight fact, not authority to bypass actual setup.

## 6. Work package D — Resource admission

Add pre-reservation required-resource checks using the same runtime capability snapshot.

Mapping:

- required memory -> `resources.cgroups-v2.memory`;
- required CPU -> `resources.cgroups-v2.cpu`;
- required PIDs -> `resources.cgroups-v2.pids`.

Semantics:

- missing required capability -> `409 capability_mismatch` before reservation/target spawn;
- best-effort requests always reach the runner and return truthful applied/not-applied evidence;
- all present required capabilities still go through the existing setup probe/verification at launch so races fail closed.

Do not convert resource rejection into `busy`, `invalid_request`, or a generic 500.

## 7. Work package E — Network admission remains fail-closed

Advertise `network.unrestricted.v1`.

Admission:

- `Unrestricted` -> accepted;
- `Disabled` -> `409 capability_mismatch`;
- `AllowListed(_)` -> `409 capability_mismatch`.

Add explicit regressions proving disabled and allow-listed modes never reach `RunnerRequest`/target spawn.

Do not add iptables/nftables/network-namespace logic in this corrective.

## 8. Work package F — Real server qualification

Extend the authenticated server/client integration fixtures.

Required Linux qualification where Landlock is available:

1. start a real `NodeServer` with required client authentication;
2. assert `/v1/capabilities` and `/v1/status` agree;
3. assert `isolation.landlock.workspace-rw.v1` is advertised;
4. materialize a workspace;
5. submit `IsolationRequirement::Required` over the production client;
6. prove a workspace read/write succeeds;
7. prove a read/write outside the workspace is denied;
8. assert terminal sandbox evidence is `Applied`;
9. assert no direct/unsandboxed fallback occurred.

Also test:

- required-isolation rejection with an unavailable/untrusted helper fixture and zero target side effect;
- best-effort with unavailable helper -> execution allowed + `NotApplied`;
- required resource rejection when a dimension is unavailable;
- unrestricted network accepted;
- disabled and allow-listed network rejected with typed `capability_mismatch`.

## 9. Work package G — Downstream compatibility fixture

Add a controller-style integration regression matching CodeGG's use:

- call `capabilities()` first;
- require `exec.argv.v1` and `isolation.landlock.workspace-rw.v1`;
- call `status()` and assert the same features;
- create/upload/materialize workspace;
- execute required-isolation argv;
- renew/cancel/observe using a fenced handle;
- verify no protocol/idempotency regression.

This is not CodeGG-specific production code; it is a substrate compatibility fixture.

## 10. Error and side-effect ordering

Capability mismatch checks for required controls MUST occur before:

- execution reservation;
- permit acquisition if practical;
- target spawn;
- workspace active-state mutation that would require terminal cleanup.

If current function ordering makes workspace authorization/resolve unavoidable first, that is acceptable, but a rejected capability request must leave no accepted execution record and no child process.

Document exact side-effect ordering in closure.

## 11. Static and architecture documentation

Update:

- `plans/subsystems/security-isolation-resource-roadmap.md`;
- security/runner architecture documentation;
- control-plane capability documentation;
- any operator documentation that currently implies Phase 5 enforcement is remotely reachable.

Add a focused static/unit guard if needed to prevent reintroducing a literal `isolation == None && network == Unrestricted` blanket gate.

Do not create a second process/setup owner.

## 12. Verification

Required:

```bash
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
git diff --check
```

Plus focused authenticated server/client, Landlock, resource, and capability-consistency suites.

Record Linux host/kernel/Landlock ABI and systemd/cgroup qualification facts exactly. Cross-compilation is not runtime qualification.

## 13. Acceptance criteria

1. Remote `Required` Landlock execution succeeds on a qualified Linux node.
2. Remote `Required` Landlock execution denies filesystem escape and reports applied evidence.
3. Required isolation fails before target spawn when unavailable.
4. Best-effort unavailable isolation reports not-applied rather than failing or claiming applied.
5. Required resource dimensions preflight/reject truthfully and still enforce at launch.
6. `/v1/capabilities` and `/v1/status` expose identical execution capability features.
7. Feature advertisement comes from runtime probes.
8. `network.unrestricted.v1` is explicit.
9. Disabled/allow-listed network remains fail-closed and never silently degrades.
10. Existing authn/authz/idempotency/lease/workspace/artifact invariants remain green.
11. No second scheduler/process/setup authority is introduced.
12. No unresolved high/medium finding remains.

## 14. Stop conditions

Stop and record a blocker if:

- Landlock capability cannot be probed truthfully without mutating the node's own security state;
- remote required isolation would need to bypass the trusted helper;
- required resource admission cannot be tied to the same runtime probes used by enforcement;
- a capability must be advertised optimistically to make tests pass;
- network-disabled semantics would require a new backend;
- the change requires weakening authentication, authorization, lease fencing, or workspace validation;
- the implementation would create a second runner/setup path.

## 15. Closure evidence

Create:

- `plans/closure/security-isolation-resource-remote-admission-corrective/001-status.md`

Record:

- implementation SHA(s);
- capability feature contract;
- capability/status equality evidence;
- Landlock runtime probe method;
- required/best-effort isolation evidence;
- required resource admission evidence;
- network unsupported negative evidence;
- authenticated client/server restricted-spec fixture;
- no-child side-effect proofs for rejection;
- full verification;
- platform qualification;
- residual findings and downstream CodeGG requalification disposition.
