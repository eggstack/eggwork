# Optional Gregg Telemetry M002 — Authenticated Node Observation API

Status: **blocked** — Eggwork telemetry M001 closure and its normalized observation contract
Class: protocol / observability
Source: `plans/subsystems/optional-gregg-host-telemetry-roadmap.md`
Decision: `plans/adrs/ADR-0006-optional-gregg-host-telemetry.md`
Reviewed baseline: `eggwork-core::NodeStatus`, `eggwork-server` node operation routes/mTLS authorization, `eggwork-client::NodeClient` (2026-10-19).

## 1. Objective

Expose M001's bounded sanitized **host-wide observation** to a specifically authorized remote controller over the existing authenticated Eggwork channel. Add a versioned typed fixed-target client method, without exposing Gregg to remote clients and without changing execution admission.

## 2. Current implementation

`NodeStatus` includes `node_id`, `draining`, `active_executions`, `NodeCapabilities`; there is no host-usage field. `eggwork-server` maps routes to `Operation`, verifies transport-derived mTLS principals, and checks `Authorizer` before responding. `eggwork-client` uses Eggfetch and targets one node. `ExecutionResult` includes categorical resource-application outcomes, not quantitative per-job measurements.

## 3. Work packages

A. Freeze an additive response contract (suggested `NodeTelemetryV1`): `schema_version`, `node_id`, `source = gregg`, typed provider state (`disabled|warming|ready|stale|unavailable|invalid`), observation timestamp and age in milliseconds, optional CPU/logical cores/utilization, optional memory available/total/used, optional load/swap and bounded disk/network aggregate rates; explicit per-metric unsupported/unknown. Never attach Gregg raw JSON. Keep the DTO protocol-neutral and non-invasive to existing `NodeStatus`. Type name/path may adapt to current crate boundaries, but document the exact wire contract for downstream CodeGG.

B. Add one bounded read route (suggested `GET /v1/node/telemetry`) to the existing Eggwork mTLS server, with existing peer principal derivation, explicit read authorization and response bounds. Verify existing status-operation grants: reuse a read-only grant only if scope is appropriate; otherwise introduce a *narrow* read grant. Do not require permission to execute jobs just to read a safe summary unless the current auth model forces a deliberate documented choice. Unauthorized principals must receive existing typed 403 without leakage. Do not add Gregg URL/proxy exposure, CORS, unencrypted fallback, or anonymous route. Requests only consult the M001 cache, not Gregg HTTP synchronously.

C. Add `NodeClient::telemetry` as an optional fixed-target method (name may adapt), using the existing Eggfetch transport/client identity. Enforce server response body cap, schema version, non-finite/out-of-range numbers, timestamp/freshness, node-id mismatch, unknown source and optional-field validation. Preserve distinguishable `unsupported` for legacy 404, `disabled` for configured-off, and `unavailable` for provider failure.

D. Keep node execution eligibility determined by the existing status/capability/preflight and server-local enforcement. Never replace node `active_executions` with a telemetry guess; do not introduce a cluster queue or worker selection. Advertise a **static protocol feature** only if necessary; avoid presenting provider readiness as an unconditional `NodeCapabilities` execution feature.

E. Update API, operations and client documentation; add a minimal operator command only if needed for diagnosing the read cache, not a new telemetry control plane.

## 4. Verification

With a stub local Gregg provider: an authenticated authorized client receives correct bounded status; unauthenticated/mismatched/unauthorized mTLS principal is denied and cannot distinguish internal metadata; an old node 404 means unsupported; a disabled provider reports disabled; Gregg failure gives unavailable without returning 500 for normal execution; remote read triggers no extra Gregg HTTP fetch; malformed and oversized response is rejected by client; actual execute/cancel/renew/lease/busy/drain semantics unaffected. Include actual loopback mTLS integration and a no-Gregg installed smoke test.

Suggested gates: `cargo test -p eggwork-core --locked`, `cargo test -p eggwork-client --locked`, `cargo test -p eggwork-server --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `python3 scripts/check_execution_ownership.py`; run repository-specified qualification as platform availability allows.

## 5. Acceptance and closure

Provide a frozen example wire payload, error taxonomy, exact operation authorization, client method, freshness semantics, reference client test and a true authenticated integration receipt. Record closure at `plans/closure/optional-gregg-host-telemetry/002-status.md`; only then mark CodeGG telemetry-consumer milestone ready. No Gregg code or config schema changes.

## 6. Stop conditions

Stop on auth bypass, need to expand Gregg, accidental direct Gregg exposure, route requiring a second HTTP server, dependency cycle, new scheduler/placement inside Eggwork, or inability to distinguish telemetry unavailable from real 0% utilization. Document these as blockers, not silent deviations.
