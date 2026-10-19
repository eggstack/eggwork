# ADR-0006: Optional Gregg Host Telemetry, Eggwork-Owned Authenticated Projection

Status: accepted
Date: 2026-10-19
Decision: Option B, explicitly approved for implementation planning.

Related authority:
- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md`
- `plans/000-long-term-specification.md`
- `plans/subsystems/optional-gregg-host-telemetry-roadmap.md`
- Downstream `dbowm91/codegg: plans/subsystems/eggwork-worker-telemetry-and-targeting-roadmap.md`

## Context

Gregg's `greggd` already provides cross-platform, cached, read-only native host measurements through `GET /v2/status` (CPU, memory, swap, load, disk and network where available). Eggwork has its own durable, authenticated fixed-target execution service, and CodeGG has the global job scheduler. The missing integration is observation, not a process supervisor or another scheduler.

## Alternatives

A. greggd supervises eggworkd / acts as an Eggwork plugin host: rejected. This makes Gregg manage a security-sensitive remote executor, duplicates Eggwork's existing service lifecycle, and expands an unauthenticated monitoring service into the execution trust boundary.

B. Eggwork optionally consumes Gregg's existing read-only status API locally and exposes a bounded authenticated node-telemetry projection: **selected**.

C. Extract Gregg's native collectors to a shared in-process library: deferred until measured idle/steady-state overhead justifies cross-repository collector refactoring.

## Decision and constraints

1. **Zero changes to Gregg.** No plugin system, callback, new route, new Rust crate, authentication feature, or new scheduler in Gregg.
2. **Eggwork remains standalone.** Default config starts no polling tasks and has no runtime dependency on Gregg. A missing, stale, malformed, overloaded or incompatible Gregg instance cannot prevent node startup, stop active executions, or weaken existing executor security.
3. **Loopback only.** The optional adapter reads a configured local `http://127.0.0.1:PORT` or `http://[::1]:PORT` endpoint (or another strictly validated loopback URI), never arbitrary URLs, redirects, proxies, non-loopback targets, userinfo or caller-supplied hostnames. Require operator configuration; no discovery of foreign greggd instances. Bound connect/read durations and response bytes; validate Gregg's v2 schema, timestamp, optional capabilities and freshness. Loopback data is **untrusted advisory input**, not remote authenticated authority.
4. **Separate observation from enforcement.** Gregg's host-wide utilization is neither per-process usage nor resource admission proof. Eggwork's existing semaphores, explicit cgroup/Landlock enforcement, drain controls and authenticated capability claims remain authoritative. Do not convert telemetry loss into `0%` usage or an invented 'idle' state.
5. **One authenticated Eggwork read surface.** Only `eggwork-server` may expose a node-level, secret-free, bounded telemetry view through the existing mTLS control plane and resource-aware authorization taxonomy. The remote controller must not connect directly to Gregg. Do not expose unredacted Gregg hostname, device identifiers, mounts, local command history, arbitrary output or process information by default.
6. **No scheduling in Eggwork.** The endpoint reports facts plus freshness/provenance; it does not score nodes, hold global reservations, delay remote jobs, choose retry targets or provide a fleet listing. CodeGG remains placement/retry authority.
7. **Compatibility is additive.** Nodes without telemetry may return a typed `unavailable/disabled` outcome or a well-defined unsupported route; older clients and nodes remain compatible. No change to existing `NodeStatus` and `NodeCapabilities` meaning merely to carry host measurements. New endpoint and optional client method should be versioned and bounded; capability advertisement cannot claim a live provider when disabled/unavailable.
8. **No secret or lease leakage.** No client certificate, lease token, private key path, Gregg URL credentials, file paths, per-job stdout or command payload enters telemetry response, debug, errors or metrics.

## Consequences

Positive: Gregg's mature native collectors are reused without editing Gregg; lean nodes need only a small optional poll/cache adapter; CodeGG can consume metrics through the existing authenticated Eggwork endpoint; Eggwork remains usable without CodeGG or Gregg.

Costs: two processes for Gregg-equipped workers and a loopback HTTP request at bounded cadence; telemetry is advisory and potentially spoofable by a local principal; cross-platform field availability differs. Mitigations: default off, loopback-only, small bounded cache, explicit uncertainty, no security-sensitive inference.

## Verification

Prove default-off startup and execution with no Gregg installed; positive v2 Gregg fixture; stale/warming/malformed/oversize/unsupported/clock-skew probes; loopback/redirect/SSRF rejections; mTLS authorized/unauthorized reads; ordinary execution under Gregg failure; isolation and resource requirements unchanged; no scheduling/pluggable executor introduced in Gregg; no increased network listener required on Gregg.

## Supersession

None. This ADR augments, and does not replace, ADR-0001 or the process-ownership and mTLS ADRs.
