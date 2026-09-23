# Control Plane M003 Closure — Eggress Route Adapter and Protocol Hardening

Status: closed

Source plan: `plans/implementation/control-plane-protocol/003-eggress-route-adapter-and-protocol-hardening.md`

Roadmap: `plans/subsystems/control-plane-protocol-roadmap.md`

## Reviewed baseline and implementation

- Reviewed parent head: `c990ffa6c0d9109fe1e3453a67c1fc88c7cc18ee`.
- Implementation commit: `48a7ac38d21bae2f41f9537a8907ff1033725d97`.
- Eggfetch: `eggfetch-core 0.2.0`, registry source inspected; public `advanced-routing` `Dialer`, `DialTarget`, and `DialStream` API.
- Eggress: `eggress-outbound 1.0.8`, registry source inspected; `OutboundConnector::connect_tcp_timeout_detailed` returns an Eggress typed error and Tokio-compatible stream. Upstream HEAD checked during implementation: `e2558f19badee3792db2fd8e6cdf71e508a7ede9`.
- Feature graph: the `eggwork-client/eggress-route` opt-in feature enables Eggfetch `advanced-routing` and optional Eggress `pproxy-compat`; the default client feature set does not enable Eggress. The test-only proxy fixtures use `eggress-testkit 1.0.8`.

## Requirement-to-evidence matrix

| Requirement | Implementation/evidence |
|---|---|
| Explicit listener-free route | `NodeClient::with_eggress` and `with_eggress_route` install one immutable Eggress connector beneath Eggfetch. The logical node HTTPS origin is retained. No local listener is created. |
| Eggfetch owns HTTP and destination TLS | Adapter returns Eggress raw TCP streams through Eggfetch `Dialer`; destination mTLS remains configured on Eggfetch. End-to-end server tests establish mTLS execution through SOCKS5 and HTTP CONNECT. |
| No fallback and one route attempt | Adapter makes one `connect_tcp_timeout_detailed` call. `route_failure_does_not_connect_direct_and_preserves_typed_error` confirms the destination listener receives no direct connection after route failure. |
| Typed/redacted route failures | Eggress kind maps to Eggfetch broad dial categories; Eggress error remains nested and is accessible through `ClientError::eggress_error`. Eggress public Debug/Display surfaces are bounded and credential-safe. |
| Protocol compatibility | Execution submission and event resume first negotiate the supported range against protocol 1.0 and require `exec.argv.v1`; incompatible versions and missing capability return separate typed client errors. Unknown event tags and oversized event records fail JSON/domain validation. |
| Bounded client event buffering | Newline-delimited event buffering is capped at 128 KiB before append; parsed event payloads run core validation. Existing server broadcast and journal limits are unchanged. |
| Direct and routed behavior | Existing direct mTLS integration remains covered. SOCKS5 and HTTP CONNECT each execute and stream stdout plus terminal success through the same client execution API. Dropping the response stream does not call the explicit cancel endpoint. |

## Verification performed

- `cargo fmt --all -- --check` — passed.
- `cargo test --workspace --all-targets` — passed after explicitly installing the ring crypto provider in the route fixture tests. This exercised 5 client tests, 12 core tests, 11 runner tests, 10 sandbox-helper integration tests, and 44 server tests, including both routed mTLS execution cases.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo check --workspace` — passed.
- Focused `cargo test -p eggwork-client --features eggress-route` — passed before the workspace run; 5 tests including SOCKS5, HTTP CONNECT, and no-direct-fallback fixtures.
- Platform qualification: Linux loopback fixtures only. Windows/macOS runtime routing is not claimed.

## Compatibility, security, and residual findings

- Direct construction remains the default and does not activate the Eggress route feature.
- Route strings are only accepted by the opt-in constructor; they are not serialized into `ExecutionSpec`. `NodeClient` Debug reports only the logical origin.
- Proxy route support is qualified for SOCKS5 and HTTP CONNECT. SSH is not enabled or claimed.
- No unresolved high/medium findings were identified in the scoped code review. Protocol-major behavior is implemented fail-closed; this closure does not claim a live incompatible-server fixture.
- No architecture or wire-domain changes were required.

## Disposition and registry

M003 is closed. The Control Plane roadmap and registry now record closure. Security M004 remains blocked until Foundation M003 and Operations M002 also close with evidence; this closure alone does not satisfy that three-workstream dependency.
