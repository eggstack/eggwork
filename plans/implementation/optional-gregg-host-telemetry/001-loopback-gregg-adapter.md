# Optional Gregg Telemetry M001 — Loopback Adapter and Bounded Cache

Status: **ready**
Class: infrastructure / observability
Source: `plans/subsystems/optional-gregg-host-telemetry-roadmap.md`
Decision: `plans/adrs/ADR-0006-optional-gregg-host-telemetry.md`
Reviewed baseline: Eggwork `main` and Gregg `docs/api.md`, `architecture/protocol.md` (2026-10-19); implementation agent must reconfirm exact current APIs and lockfile before editing.
Dependency: Gregg's existing v2 status contract only; **no Gregg code changes**.

## 1. Objective

Build an opt-in, read-only host telemetry adapter behind Eggwork-server that consumes local Gregg v2 cached status and produces a bounded typed host observation with explicit provenance, freshness and unavailable reasons. M001 does not add any remote endpoint or modify admission.

## 2. Existing implementation evidence

- `eggwork-core::NodeStatus` has only node id, draining, active-execution count and capabilities.
- Eggwork already has a supervised `NodeServer` lifetime and stable Eggfetch transport dependencies.
- Gregg's `GET /v2/status` includes `observed_at_unix_ms`, `sample_interval_ms`, `capabilities`, CPU, memory and optional load/swap/drives/disk_io/network; unknown optional fields are not zero. Gregg's `gregg-protocol` defines validation; its regular HTTP service is unauthenticated.
- Gregg's server may bind `0.0.0.0` by default. Document and require loopback binding operationally; Eggwork should only contact a verified loopback endpoint.

## 3. Production work packages

A. Confirm Gregg payload/v2 validation API and the existing Eggwork HTTP client abstraction. Implement a minimal `HostTelemetryProvider`-style internal seam in `eggwork-server` (or equivalent sibling) with a disabled default, one Gregg implementation, and a fake for tests. Keep Eggwork-core protocol-neutral and free of Gregg types. If `gregg-protocol` materially increases runtime/dependency cost, map a minimal DTO with equivalent strict validation instead; record the measured reason and parity tests. **Do not fork Gregg's collectors.**

B. Add opt-in node configuration for a loopback HTTP endpoint, refresh interval and freshness TTL with safe bounded defaults. Validate complete origin, port, host/IP literal, scheme and path; reject userinfo, HTTPS/TLS ambiguity, DNS/hostnames (including localhost unless resolved safely without rebinding), non-loopback IP, fragments, redirect hops, inherited proxies and request-derived URLs. Require fixed path `/v2/status`. No auto-discovery or new listener.

C. Use one node-scoped refresh owner and cache, never one Gregg poll per remote caller. Bound connect timeout, overall deadline, decompressed body bytes, JSON collections and concurrent refreshes. Treat Gregg as advisory input. No locking across network await; no launch-time wait or blocking on Gregg. Validate schema v2, timestamps, counter units, finite percentages, capability/value agreement, sample interval, missingness. Differentiate disabled, warming, unavailable, stale, malformed and valid-with-unsupported-field; never substitute zero for missing measurements. Use monotonic age plus wall-clock provenance and bounded clock-skew tolerance; do not accept future timestamps as indefinitely fresh.

D. Normalize only host-wide measurements needed by scheduling: CPU cores/utilization, memory bytes/usage, optional swap/load, available disk, disk and network rates, with optional support flags and a source/staleness summary. Avoid transmitting raw mount paths, interface identifiers, hostnames, local jobs, command output or other Gregg data beyond the server. Do not claim per-execution utilization. Make the internal response type ready for future mTLS projection.

E. Lifecycle and logging: create no task when disabled; when enabled, bounded refresh with backoff and jitter or fixed bounded interval; cancelled on node shutdown. Gregg errors are categorized and rate-limited, not logged with arbitrary bodies/paths. Normal remote execute/cancel/status must remain independent.

## 4. Verification

Focused deterministic tests with fake HTTP Gregg v2 responses: valid minimal/full/optional/missing fields, v1-only/404, warming, 503, malformed, duplicate/oversized fields, response slowloris/body cap, negative/future/stale timestamps, clock rollback, unsupported OS fields, SSRF and URL/proxy/redirect rejection, multiple readers one refresh, and shutdown with in-flight fetch. Assert no network calls/default-off, no process spawn, no mutation to Eggwork admission, and isolation/resource enforcement unchanged.

Use the narrow crate checks as applicable: `cargo fmt --all --check`; `cargo test -p eggwork-server --locked`; `cargo clippy -p eggwork-server --all-targets --locked -- -D warnings`; `python3 scripts/check_execution_ownership.py`; broader repository CI gates/target matrix as instructed by `CONTRIBUTING.md`. Verify exact command names at handoff. An optional real greggd loopback smoke test is operational evidence, never a substitute for negative fixtures.

## 5. Compatibility, docs and acceptance

No wire change, new required runtime dependency, database migration, new service manager, or Gregg modification. Add operator docs showing **Gregg must be bound to loopback** and that default `eggworkd` runs without Gregg. M001 closes only with typed error/freshness semantics, bounded single-owner polling, strict local origin enforcement, default-off proof, tests and source-level footprint/dependency note. Deliver a closure record at `plans/closure/optional-gregg-host-telemetry/001-status.md`.

## 6. Stop conditions / handoff

Stop and record a blocker if the Gregg protocol cannot be consumed read-only without widening Gregg; if the adapter requires a new network listener, unauthenticated Eggwork entry point, process spawn, altered admission, or broadened `eggwork-core` dependency direction; or if chosen HTTP client cannot reliably disable redirects/proxies and enforce loopback. Do not silently widen this milestone to M002.
