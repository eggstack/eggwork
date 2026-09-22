# ADR-0002: Eggstack HTTP Transport with Transport-Derived mTLS Identity

Status: accepted

Date: 2026-09-22

Decision owners: project maintainers

Related specification:

- `plans/000-long-term-specification.md#13-security-model`
- `plans/000-long-term-specification.md#15-transport-and-eggstack-ownership`
- `plans/000-long-term-specification.md#16-protocol-surface`

External interface baselines at planning time:

- Eggfetch 0.2.0 line
- Eggress 1.0.8 line
- EggServe 0.2.0 line with reusable `eggnet-tls`
- CodeGG baseline recorded in `plans/registry.md`

These are research baselines, not permanent pins.

## Context

Eggwork requires authenticated remote arbitrary-process execution, streaming output/events, cancellation, and large content transfer.

Eggstack already contains the relevant low-level pieces:

- EggServe: bounded HTTP service, streaming responses, cancellation-aware lifecycle, TLS termination, mTLS, generic tunnel seams;
- Eggfetch: async HTTP/TLS client, streaming bodies, client certificates, custom/additive trust roots, advanced routing seams;
- Eggress: listener-free outbound proxy-chain connection establishment with typed failure classification;
- eggnet-tls: reusable identity/trust/client-authentication substrate.

Introducing an independent gRPC/HTTP stack would duplicate substantial code and maintenance.

## Decision drivers

- reuse maintained Eggstack components;
- avoid a second HTTP/TLS implementation;
- strong authenticated identity before command execution;
- stream events and blobs with backpressure;
- retain proxy/SSH/multi-hop support without embedding it in Eggwork;
- keep identity out of untrusted request payloads;
- allow future protocol evolution without requiring protobuf/gRPC as an initial dependency.

## Considered options

### Option A — Custom TCP binary protocol

Potentially compact and controllable, but would require new framing, TLS integration, multiplexing, cancellation, backpressure, tooling, and diagnostics.

Rejected for v1.

### Option B — gRPC/protobuf

Mature RPC semantics but introduces a separate transport stack and code-generation/schema ownership not otherwise required by Eggstack.

Deferred. A future adapter is possible if demanded by interoperability.

### Option C — HTTP/1.1 streaming using EggServe/Eggfetch, mTLS identity, optional Eggress route

Selected.

## Decision

1. Initial network protocol uses versioned HTTP resources over EggServe/Eggfetch.
2. HTTP/1.1 is the required first transport. H2/H3 are future capabilities, not prerequisites.
3. Remote execution uses TLS by default; mTLS is the initial recommended normal authentication model.
4. The server maps verified transport/session identity to an Eggwork principal. Request bodies do not carry authoritative principal/role/capability fields.
5. Eggfetch owns client HTTP/TLS behavior. EggServe owns inbound HTTP service/cancellation/backpressure/TLS surfaces.
6. Eggress MAY supply the client's route/dial path for direct, HTTP CONNECT, SOCKS, SSH, or multi-hop transport where a supported public seam exists.
7. Eggress route failure must remain a failure. Eggwork must not silently retry direct.
8. JSON is the control-object format for v1.
9. Blob transfer uses streaming bytes rather than base64 in control JSON.
10. Execution events initially use a simple bounded streaming representation such as NDJSON or an equivalent small framed stream.
11. Protocol semantics live in Eggwork domain types; the HTTP route layout is an adapter and can evolve behind version negotiation.
12. Normal network listeners fail closed if the configured authentication requirement cannot be initialized.

## Consequences

### Positive

- large reuse of existing Eggstack code;
- good testability with local loopback fixtures;
- standard observability/debugging;
- mTLS identity is available before request deserialization reaches side-effectful operations;
- no new generic proxy implementation.

### Negative

- H1 provides less native multiplexing than H2/gRPC;
- long-lived event streams require careful connection/backpressure handling;
- some Eggress/Eggfetch integration may require a published custom-dial interface to line up cleanly.

## Security implications

- payload-supplied identity is untrusted metadata only;
- peer certificate material must not be dumped into ordinary logs;
- certificate/trust configuration must be bounded and validated before readiness;
- authorization happens after transport identity resolution but before admission or process/workspace side effects.

## Verification

- local test CA with valid/invalid client certificates;
- unauthenticated client produces zero execution side effect;
- verified peer maps deterministically to principal;
- event and blob streams exert backpressure;
- disconnect/cancellation semantics are explicit;
- Eggress route errors retain typed route failure without direct fallback;
- secrets do not appear in debug/error serialization.

## Supersession

None.
