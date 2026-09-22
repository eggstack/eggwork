# Control Plane M001 Closure — Authenticated Fixed-Target Execution

Source plan: `plans/implementation/control-plane-protocol/001-authenticated-fixed-target-execution.md`  
Subsystem roadmap: `plans/subsystems/control-plane-protocol-roadmap.md`  
Reviewed head: `6c6b6e6c227b90dba61dff61a22c82b426df569e`  
Implementation commits: `6293585` (server/client API), `6c6b6e6` (bounds and lifecycle tests), `e6b7e22` (stream and cancellation semantics)

## Finding

M001 is closed for the exercised Linux host. `eggwork-server` exposes a fixed-target EggServe service over TLS with required client authentication, maps EggServe's verified leaf certificate to a server-owned principal, authorizes each operation before handler side effects, and immediately admits bounded local executions through the canonical runner. `eggwork-client::NodeClient` uses Eggfetch with an explicit HTTPS endpoint, CA, and client identity. It exposes capabilities, status, execute/live events, observe, cancel, and event attachment for one node. There is no execute-anywhere, retry, queue, or node-selection API.

Execution output uses bounded runner and broadcast channels, bounded request bodies, bounded event chunks, and a capped recent-execution map. A lagging live consumer gets a stream error while execution proceeds to its authoritative terminal snapshot. Dropping the initial event stream does not cancel execution; cancellation is explicit. The node cancels active runs at shutdown and waits for terminal cleanup before `wait` returns. Durable idempotency, leases, restart recovery, and event resume remain M002.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| EggServe/Eggfetch public 0.2 APIs and no second HTTP stack | `eggwork-server` uses `eggserve-core 0.2.0`; `eggwork-client` uses `eggfetch-core 0.2.0`. `cargo tree` confirmed `eggnet-tls 0.2.0` through EggServe. No direct Hyper/Rustls transport implementation was added. Published APIs provide EggServe TLS peer-chain context and Eggfetch TLS/client streaming. |
| Fail-closed remote mTLS listener | `NodeServer::start` rejects TLS configs unless client authentication is Required and a default server identity is configured. Runtime exposes the verified peer chain. Loopback tests use an ephemeral CA and server/client leaf certificates. Valid client succeeds; no client certificate and a client certificate from an untrusted CA fail at TLS; a trusted but unmapped principal receives 401. |
| Principal mapping and authorization before execution side effects | The resolver fingerprints only EggServe's verified leaf DER. The principal is never read from request JSON. An authorized TLS client denied Execute receives 403 and a marker-file command leaves no file. |
| Typed validation and request bounds | Tests exercise malformed JSON (400), an over-limit body (413), and unsupported protocol schema (426). Both EggServe's runtime body ceiling and service buffering policy are set to 1 MiB. Execution specs are validated before admission; unsupported isolation/network requirements return a capability mismatch. |
| Fixed-target client protocol | Client API is `NodeClient` bound to one HTTPS URL. It provides capabilities/status, execute on that URL, observe, cancel, and events. Route test confirms `/v1/execute-anywhere` is absent. Eggfetch is configured without retry or redirect features, and the client contains no semantic resubmission. |
| Immediate admission and drain behavior | Loopback test sets the active cap to one, starts a long process, and confirms a second execution receives 503 busy. Drain mode returns 503 draining without queueing. Recent execution records are capped at 1024; terminal records are evicted to make room and all-active capacity returns storage-exhausted. |
| Output/event/result lifecycle | Loopback execution streams stdout as NDJSON events, then `observe` returns a succeeded snapshot with result. A disconnected stream is dropped while a long process continues to success. The client reattaches through `GET /events` and receives terminal events. Explicit cancel reaches Cancelled. Shutdown cancels an active run and `wait` completes after its terminal snapshot. Per-execution event sequences start at one and publish in order. |
| Bounded streaming/backpressure | Runner output uses its bounded channel and nonblocking pipe-drain path; the live event broadcast capacity is 32. Event chunks are bounded by the validated execution output policy. A deterministic slow-consumer test exceeds broadcast capacity and verifies a typed stream error while further producer sends remain nonblocking. The final snapshot remains authoritative. |
| Failure and secret handling | API errors use fixed, bounded messages and do not include request bodies, certificate chains, private keys, or command/environment values. Authentication and validation denials do not invoke the runner. This implementation emits no credential-bearing logs. Eggfetch owns transport error redaction. |

## Verification actually run

Pinned toolchain: repository Rust 1.89.0.

- `cargo fmt --all` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed (18 tests across workspace targets; RTK reported 4 suites).
- `cargo tree -p eggwork-server -p eggwork-client -i eggserve-core` — confirmed EggServe 0.2.0.
- `cargo tree -p eggwork-server -p eggwork-client -i eggfetch-core` — confirmed Eggfetch 0.2.0.
- `cargo tree -p eggwork-server -i eggnet-tls` — confirmed Eggnet TLS 0.2.0 via EggServe.
- `git diff --check` — passed before closure commit.

## Compatibility, security, and platform review

- Only Linux loopback execution was exercised. Other Unix platforms and Windows are not qualified by this closure; process behavior remains delegated to the foundation runner.
- The HTTPS client validates the configured CA and hostname. Its constructor rejects non-HTTPS endpoints, credentials, queries, and fragments. No client-side automatic retry or redirect feature is enabled.
- TLS authentication proves certificate-chain membership; the configured resolver separately decides which verified leaves map to Eggwork principals. Unknown trusted leaves are denied at the API layer.
- Client disconnect means stream disconnect only. The execution continues unless the caller invokes cancel or the node shuts down.
- Residual scope: `GET /events` attaches to the live bounded channel; it does not replay prior events or resume from a cursor. Retention/resume is M002. No high/medium finding blocks M001 closure.
- Eggress outbound routing was outside this milestone and remains a later Control Plane M003 compatibility check.
- Disposition: **closed for the exercised Linux host**.

## Registry/roadmap and next-plan disposition

Control Plane M001 is closed. Control Plane M002's only registered hard dependency is now satisfied and it is promoted to active. Security M001's registered hard dependencies (Foundation M001 closure and availability of the Control Plane M001 authorization interface) are both satisfied, so that plan is promoted to ready; it remains inactive until the requested sequence reaches it. Workspace M001 remains blocked on Control Plane M002 closure. No later plan is promoted past its remaining dependency.
