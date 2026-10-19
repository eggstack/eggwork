# Optional Gregg Host Telemetry — Subsystem Roadmap

Status: closed — M001 and M002 both closed; the CodeGG worker-telemetry consumer is unblocked
Primary class: infrastructure / observability
Decision: `plans/adrs/ADR-0006-optional-gregg-host-telemetry.md`
Governing references: `plans/000-long-term-specification.md`, `plans/001-terminology-and-domain-model.md`, `plans/003-planning-process.md`, ADR-0001/0002/0005.
Downstream: `dbowm91/codegg: plans/subsystems/eggwork-worker-telemetry-and-targeting-roadmap.md`.

## Purpose and ownership

Reuse an already installed local greggd for **host-wide, advisory resource measurements**. Eggwork-server owns a default-off adapter/cache and a bounded mTLS-authenticated node telemetry response. Gregg remains unmodified and independently operated. Eggwork-runner alone owns process execution and resource enforcement. CodeGG or another consumer owns scheduling and placement.

## Current baseline (2026-10-19)

- `greggd` exposes `GET /v2/status` with timestamp, CPU, memory, optional load/swap/drive/disk I/O/network and capability flags (see `eggstack/gregg: docs/api.md` and `architecture/protocol.md`). Its listener is unauthenticated, configurable, and may otherwise bind to all interfaces: when deployed for Eggwork use, operator must bind it to loopback.
- Eggwork `NodeStatus` exposes node_id, draining, active_executions and `NodeCapabilities` (not host usage), `eggwork-core::ExecutionResult` exposes categorical resource setup outcomes (not quantitative per-attempt CPU/RSS).
- `eggwork-server` has mTLS principal resolution, operation authorization and local protective admission; `eggwork-client` is fixed-target and uses Eggfetch.
- M001 is closed: `eggwork-server` has a default-off loopback Gregg adapter, a normalized in-memory cache, and a single polling owner (`plans/closure/optional-gregg-host-telemetry/001-status.md`, implementation `ac55985`). The normalized observation is `eggwork_core::HostTelemetry`.
- M002 is closed: the frozen wire contract is `eggwork_core::NodeTelemetry` (schema version 1), served at `GET /v1/node/telemetry` behind a narrow `telemetry` grant, with `NodeClient::telemetry` / `telemetry_for` on the client (`plans/closure/optional-gregg-host-telemetry/002-status.md`, implementation `5ccb1e6`). `NodeStatus` and `NodeCapabilities` are unchanged, and no capability feature was added.
- No new scheduler, node discovery, global worker selection or control authority is justified.

## Invariants and non-goals

Keep the existing Eggwork transport/authorization, controller identity, lease fencing, resource guarantees, node-local admission, executable ownership and drain semantics. Gregg must require no code changes and must not become a subprocess supervisor. No live Gregg network API exposure to remote controllers; no unauthenticated Eggwork status extension. No fabricated zero/idle metrics, per-execution attribution, or automatic worker placement.

## Dependency graph

```text
greggd existing v2 API (external, no changes)
             |
             v
 Eggwork M001 local read-only adapter/cache [CLOSED]
             |
             v
 Eggwork M002 authenticated telemetry endpoint/client [CLOSED]
             |
             v
 CodeGG worker telemetry consumer [interface-dependent; separately registered]
```

## Milestones

**M001 — Optional loopback Gregg adapter and normalized cache (closed).**
Implementation: `plans/implementation/optional-gregg-host-telemetry/001-loopback-gregg-adapter.md`.
Closure: `plans/closure/optional-gregg-host-telemetry/001-status.md` (implementation `ac55985`).
Exit: zero-default-overhead; validated and bounded native Gregg v2 parsing (prefer existing `gregg-protocol` only if dependency impact is reasonable); typified snapshot with sampling/source/staleness/capability facts; no subprocesses; loopback-only; explicit unknown, warming, stale and invalid conditions. Production node execution remains unaffected. **Met.** `gregg-protocol` 1.0.15 was used: three added packages, no new dependency family, and the provider's own validation rules rather than a copy of them. The frozen normalized contract is `eggwork_core::HostTelemetry` with `TelemetryProviderState` and `TelemetryErrorCategory`.

**M002 — Authorized Eggwork projection and fixed-target client method (closed).**
Implementation: `plans/implementation/optional-gregg-host-telemetry/002-authenticated-node-telemetry.md`.
Closure: `plans/closure/optional-gregg-host-telemetry/002-status.md` (implementation `5ccb1e6`).
Exit: an authorized controller reads a bounded sanitized snapshot through mTLS; an unauthorized principal is refused, and an older node is reported unsupported; no Gregg endpoint reachable or required remotely. Node status, capabilities, execution semantics and error authority unchanged. **Met.** The narrow `telemetry` grant is separate from `status`, so no existing grant widened; the route reads the M001 cache only, so a remote read never becomes a provider request; and the client refuses a response it cannot trust.

## Cross-cutting concerns

- Security: trust boundary terminates at Eggwork mTLS; Gregg loopback data is never enforcement evidence; capability-based authorization precedes any sensitive response.
- Compatibility: default off, additive opt-in config and response DTO; rollback by disabling provider. No database migration required for cached observations.
- Runtime: one loop per node at most, monotonic refresh/freshness deadlines, bounded CPU/network work, no spin if Gregg is down; avoid client-query-amplified Gregg requests.
- Platform: Gregg v2 optional OS fields accepted, no invented metrics. Windows/macOS/linux qualification must match actual hosted evidence.
- Observability: source, observed age, capability/field missingness and last error category; exclude raw host strings and command history.
- Risks: local spoofing, clock skew, unexpectedly huge JSON, SSRF/proxy/redirect, polling contention and exposure of operational metadata.

## Deferred

Shared Gregg collector crate, UDS authorization, time-series persistence, per-execution cgroup accounting, automatic selection and resource-aware reservations belong to independent decisions/workstreams. Revisit collector extraction only with measured footprint evidence.

## Completion

Both milestones closed with code/test receipts and verified default-off no-Gregg execution; the CodeGG consumer can depend on a stable, versioned authenticated response. No Gregg commits are created.

**Met.** M001 closed at `ac55985` (`plans/closure/optional-gregg-host-telemetry/001-status.md`) and M002 closed at `5ccb1e6` (`plans/closure/optional-gregg-host-telemetry/002-status.md`). The frozen contract is `eggwork_core::NodeTelemetry`, schema version 1, documented in `architecture/protocol.md`. The default-off no-Gregg execution is proven twice: by the installed-release qualification receipt and by an installed-binary probe of the route itself. The CodeGG worker-telemetry consumer is unblocked and may now be registered against this contract.
