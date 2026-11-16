# Control Plane M003 — Eggress Route Adapter and Protocol Compatibility Hardening

Status: closed

Source roadmap:

- `plans/subsystems/control-plane-protocol-roadmap.md`

Canonical/ADR references:

- `plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md`
- `plans/adrs/ADR-0002-eggstack-transport-and-mtls.md`
- `plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md`

Current interface baselines reviewed for this plan:

- Eggwork `86f80d6c0ce5818b11ed272847656a5813f726c2`;
- Eggfetch `b90b32541bd5dac6c5256feed141cadfd124debe`, `eggfetch-core 0.2.0` advanced-routing `Dialer` surface;
- Eggress `e141d4082d211cc5f122414c74617fde846ffbae`, listener-free `eggress-outbound::OutboundConnector`.

Implementation must re-check the exact current public APIs before changing dependencies.

## 1. Objective

Add an optional listener-free Eggress route beneath the existing fixed-target Eggwork client while preserving Eggfetch ownership of HTTP and destination TLS, and close the remaining protocol-version/backpressure compatibility gaps.

No local proxy listener, route fallback, node selection, or semantic retry is permitted.

## 2. Verified composition seam

At plan authoring, Eggfetch exposes:

- public `Dialer` / `DialTarget` / `DialStream`;
- `ClientBuilder::dialer(...)` under `advanced-routing`;
- Eggfetch-owned HTTP framing and destination TLS/SNI/certificate verification above the custom raw stream;
- fail-closed incompatibility with competing built-in route modes.

Eggress exposes:

- `OutboundConnector::connect_tcp_detailed(host, port)`;
- a Tokio-compatible boxed byte stream;
- typed `OutboundConnectError` with kind/stage/hop/protocol;
- no direct fallback after a configured proxy-chain failure.

That is sufficient to remove the previous interface blocker.

## 3. EggressDialer adapter

Implement a small Eggwork-client-owned adapter that:

1. holds an `Arc<OutboundConnector>` or equivalent immutable route object;
2. implements Eggfetch `Dialer`;
3. calls `connect_tcp_detailed` exactly once for each Eggfetch dial;
4. returns the Eggress stream as the Eggfetch raw dial stream;
5. maps broad failure category to `DialErrorKind`;
6. retains the typed Eggress error as the nested source where safe;
7. uses a bounded, credential-redacted public message;
8. never retries direct.

Do not reimplement SOCKS, CONNECT, SSH, TLS-to-proxy, chain execution, or DNS routing inside Eggwork.

## 4. Route construction surface

Add an explicit client construction path, for example a route enum/builder or `NodeClient::with_eggress(...)`.

Requirements:

- direct construction remains the default/current path;
- Eggress routing is explicit and opt-in;
- route configuration is not serialized into ExecutionSpec;
- route credentials are never included in `Debug`/ordinary errors;
- the selected Eggwork node endpoint remains the logical HTTPS origin;
- Eggfetch still performs mTLS to the Eggwork node after Eggress establishes the route.

Do not allow a failed Eggress route to instantiate a direct client automatically.

## 5. Feature/dependency shape

Prefer optional features so consumers that do not need routed transport do not acquire Eggress's full protocol surface.

Use the smallest Eggress feature set needed for the qualified paths.

At minimum qualify local fixtures for:

- SOCKS5;
- HTTP CONNECT.

SSH support may be exposed only when the corresponding Eggress feature is enabled and a truthful test/compile surface exists. Do not claim SSH runtime qualification from feature compilation alone.

Do not enable Eggfetch logical retry merely to support the adapter.

## 6. Typed route failure preservation

Define Eggwork-client error access so callers can distinguish at least the broad route class without parsing display text.

Where the nested Eggress type can remain available without forcing it into Eggwork core/wire types, preserve:

- failure kind;
- stage;
- hop index;
- protocol label.

These are diagnostic facts, not retry recommendations.

No Eggress-specific type should enter `eggwork-core::ExecutionFailure` because route failure occurs before/around the client transport, not inside the remote execution domain.

## 7. Protocol compatibility hardening

Review the current `ProtocolVersion` / capability handshake and make client behavior explicit.

Required behavior:

- incompatible major version fails before execution submission;
- supported minor/additive capability differences are negotiated conservatively;
- capability absence produces a typed client-side unsupported/capability error;
- reconnect/resume uses the same execution generation and protocol compatibility rules;
- the client cannot silently interpret an unknown terminal/event semantic as success.

Do not create a cluster-level negotiation service.

## 8. Slow-consumer/backpressure review

Re-run the event-stream path after routed transport is introduced.

Prove:

- client/event buffering remains bounded;
- server broadcast/event journal bounds remain unchanged;
- route latency/slow reads cannot create unbounded queued chunks;
- history-expired/resync behavior still works through the routed path;
- dropping a routed event stream does not implicitly cancel the execution.

## 9. Tests

Use deterministic local fixtures where possible:

- direct client control case;
- SOCKS5-routed mTLS execution;
- HTTP CONNECT-routed mTLS execution;
- route auth failure;
- hop handshake/connect failure;
- route timeout;
- wrong node certificate through a successful proxy route;
- proxy failure with assertion that no direct connection occurred;
- cancellation and lease renewal over routed connection;
- event resume/history-expired over routed connection;
- incompatible protocol major;
- additive minor/capability negotiation;
- slow routed consumer/backpressure;
- Debug/error credential-negative assertions.

If Eggress testkit exposes suitable fixtures, consume it rather than creating a second proxy implementation.

## 10. Acceptance criteria

1. Eggwork can reach one explicitly selected node through an in-process Eggress TCP route.
2. Eggfetch remains owner of HTTP and destination mTLS.
3. No loopback proxy service is created.
4. Proxy failure never falls back direct.
5. Typed route failure information is recoverable without string parsing.
6. Direct and routed clients exercise equivalent execution/idempotency/lease semantics.
7. Protocol major mismatch fails before submission.
8. Event/backpressure bounds remain intact.
9. Direct-only consumers do not require the routed feature/dependencies.
10. No unresolved high/medium finding remains.

## 11. Stop conditions

Stop and report if:

- the current Eggfetch Dialer cannot preserve destination TLS/SNI correctly;
- Eggress stream types cannot satisfy Eggfetch's public DialStream safely;
- support would require a local listener;
- route errors can only be preserved by leaking credentials;
- implementing routed transport would require automatic semantic retries or node selection.

## 12. Closure evidence

Create `plans/closure/control-plane-protocol/003-status.md` containing:

- exact Eggfetch/Eggress versions/SHAs used;
- feature graph;
- direct/SOCKS5/CONNECT fixture evidence;
- TLS ownership evidence;
- no-fallback negative test;
- typed failure mapping matrix;
- protocol compatibility matrix;
- slow-consumer/resume evidence;
- full workspace verification;
- residual platform/protocol findings.
