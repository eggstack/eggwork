# Optional Gregg host telemetry — M002 closure

Status: **closed**
Closed: 2026-10-19
Source plan: `plans/implementation/optional-gregg-host-telemetry/002-authenticated-node-telemetry.md`
Subsystem roadmap: `plans/subsystems/optional-gregg-host-telemetry-roadmap.md`
Decision: `plans/adrs/ADR-0006-optional-gregg-host-telemetry.md`
Branch: `planning/optional-gregg-telemetry-m001-m002`
Implementation commit: `5ccb1e6` — *feat(protocol): authorized node telemetry read route and client method (M002)*
Reviewed head at closure: `5ccb1e6`
Predecessor: M001 closed at `2190818` (implementation `ac55985`)

## Executive finding

M002 is implemented and closed. An authorized controller can now read the
node's bounded, sanitized host-wide observation over the existing mTLS
channel, and the provider itself remains where it was: on the node's
loopback interface, reachable by nobody remotely.

The three properties the milestone was written to establish are established:

1. **The provider is not remotely reachable.** The route is a read of the
   node's own cache. It never contacts the provider, so a burst of readers
   cannot become a burst of provider requests, and a provider that is down
   does not turn a read into a timeout.
2. **Authorization is narrow and explicit.** The route is guarded by a
   separate `telemetry` grant, not folded into `status`. A principal without
   it receives the existing typed 403, and the refusal reveals nothing about
   whether the node holds an observation.
3. **The contract is frozen and the client does not trust it blindly.** The
   wire document is versioned, its `state`/`observation` pair is documented
   and checked, and the client refuses a response that is malformed,
   oversized, of an unknown version or source, self-contradictory, out of
   range, or addressed to a different node.

The node's execution semantics are unchanged. `NodeStatus` and
`NodeCapabilities` keep their existing meaning, no capability feature was
added, admission is untouched, and no new scheduler, queue, or placement
authority was introduced.

## The frozen wire contract

`GET /v1/node/telemetry` returns `eggwork_core::NodeTelemetry`, schema
version 1. The contract is written out in full in
[`architecture/protocol.md`](../../architecture/protocol.md#the-node-telemetry-wire-contract-schema-version-1).

```json
{
  "schema_version": 1,
  "node_id": "worker-01",
  "source": "gregg",
  "state": "ready",
  "age_ms": 231,
  "last_error": null,
  "observation": {
    "observed_at_unix_ms": 1729460000123,
    "sample_interval_ms": 1000,
    "cpu": { "value": { "logical_cores": 8, "usage_pct": 12.5, "iowait_pct": 1.5 } },
    "memory": { "value": { "available_bytes": 4000000, "total_bytes": 8000000, "used_bytes": 4000000 } },
    "load": { "value": { "one": 0.5, "five": 0.25, "fifteen": 0.125 } },
    "swap": "unsupported",
    "disk": { "value": { "available_bytes": 900, "total_bytes": 1000, "volume_count": 1 } },
    "disk_io": { "value": { "read_bytes_per_sec": 11, "write_bytes_per_sec": 22 } },
    "network": { "value": { "rx_bytes_per_sec": 33, "tx_bytes_per_sec": 44 } }
  }
}
```

### State taxonomy

`state` describes the **provider**, never the host.

| `state` | `observation` | Meaning |
|---|---|---|
| `disabled` | absent | No provider is configured. The node reports no host measurements. |
| `warming` | absent | The provider is alive but has not produced a sample yet. |
| `ready` | present | A sample is present and within its freshness bound. |
| `stale` | present | A sample is present but older than the freshness bound. |
| `unavailable` | present or absent | The provider could not be reached. |
| `invalid` | present or absent | The provider answered with something unusable. |

A sample is **current only when the state is `ready`**. `unavailable` is not
zero utilization. The per-measurement groups keep the rest explicit:
`unsupported` means the platform cannot produce the measurement, `unknown`
means it can but this sample carried none, and only `value` is a
measurement.

### Error taxonomy

Provider failures are reported as a bounded category and never as prose:
`connection`, `timeout`, `http_status`, `malformed`, `schema_version`,
`invalid`, `too_large`, `clock_skew`. The client maps a node that has no
such route to `ClientError::TelemetryUnsupported`, and an unauthorized
principal to the node's existing typed 403. Those three answers — *the node
cannot serve telemetry*, *you may not read it*, *the provider is down* — are
distinguishable without parsing a message.

## What landed

| Area | Change |
|---|---|
| `crates/eggwork-core/src/lib.rs` | `NodeTelemetry` (the frozen wire DTO), `NODE_TELEMETRY_SCHEMA_VERSION`, and `NodeTelemetry::validate()`: schema version, the `state`/`observation` agreement, the `observation`/`age_ms` agreement, and the delegated measurement validation. |
| `crates/eggwork-server/src/lib.rs` | `Operation::Telemetry`, `Route::Telemetry`, the `GET /v1/node/telemetry` route entry, the dispatch arm, and the `node_telemetry` projection (a cache read, nothing else). |
| `crates/eggwork-server/src/operations.rs` | The `telemetry` grant name. |
| `crates/eggwork-client/src/lib.rs` | `NodeClient::telemetry` and `NodeClient::telemetry_for`, `ClientError::TelemetryUnsupported`, and the 64 KiB telemetry body cap. |
| `architecture/protocol.md` | The frozen contract, written out in full. |
| `architecture/server-node.md` | The read route, the narrow grant, and the 404/403 distinction. |
| `architecture/client-transport.md` | The client method, its refusals, and the new error variant. |
| `docs/operations.md` | Operator-facing: the `telemetry` grant, the route, and the state table. |
| `examples/eggworkd.example.json` | The `telemetry` operation name in the grant list. |

## Requirement-to-evidence matrix

| Plan requirement | Evidence | Where |
|---|---|---|
| A. Freeze an additive response contract; never attach Gregg raw JSON; keep the DTO protocol-neutral and non-invasive to `NodeStatus` | `NodeTelemetry` + `NODE_TELEMETRY_SCHEMA_VERSION`; `a_ready_telemetry_response_validates`, `a_telemetry_response_with_an_unknown_schema_version_is_refused`, `a_ready_telemetry_response_without_an_observation_is_refused`, `a_disabled_telemetry_response_with_an_observation_is_refused`, `a_telemetry_response_whose_age_contradicts_its_observation_is_refused`, `a_telemetry_response_round_trips_through_json` | `crates/eggwork-core/src/lib.rs` tests |
| B. One bounded read route with existing peer principal derivation, explicit read authorization, and response bounds | `an_authorized_principal_reads_the_cached_observation`; `an_unauthorized_principal_cannot_read_the_observation`; `route_contract_is_fixed_target_and_versioned` (the route is in the fixed table) | `crates/eggwork-server/src/lib.rs` tests |
| B. Reuse a read-only grant only if the scope is appropriate; otherwise introduce a narrow read grant | The `telemetry` grant is separate from `status`. The installed-binary receipt below shows a principal **without** it receiving 403 on the telemetry route while its `status` read still returns 200. | installed-binary probe; `docs/operations.md` |
| B. Unauthorized principals receive the existing typed 403 without leakage | `an_unauthorized_principal_cannot_read_the_observation` asserts the 403 and that the rendered error contains neither `logical_cores` nor `usage_pct`; the installed-binary probe shows `{"code":"forbidden","message":"operation is not authorized"}` | as above |
| B. No Gregg URL/proxy exposure, CORS, unencrypted fallback, or anonymous route | The route is served by the same mTLS-only EggServe surface as every other route; the unauthenticated probe never reaches HTTP (the TLS handshake is refused). The response carries no provider address. | installed-binary probe; `an_authorized_principal_reads_the_cached_observation` |
| B. Requests only consult the M001 cache, not Gregg HTTP synchronously | `an_authorized_principal_reads_the_cached_observation` asserts the provider request count is unchanged across 16 remote reads | `crates/eggwork-server/src/lib.rs` tests |
| C. A fixed-target client method using the existing Eggfetch transport and client identity | `NodeClient::telemetry` uses the same `HttpClient` and `TlsConfig` as every other method | `crates/eggwork-client/src/lib.rs` |
| C. Enforce body cap, schema version, non-finite/out-of-range numbers, timestamp/freshness, node-id mismatch, unknown source, optional-field validation | `an_oversized_body_is_refused`, `an_unknown_schema_version_is_refused`, `an_unknown_source_is_refused`, `a_response_with_an_out_of_range_measurement_is_refused`, `a_response_whose_number_does_not_fit_is_refused`, `a_response_whose_state_contradicts_its_observation_is_refused`, `a_malformed_body_is_refused`, and the `telemetry_for` node-id mismatch assertion | `crates/eggwork-server/src/lib.rs` tests |
| C. Preserve distinguishable `unsupported` (legacy 404), `disabled`, and `unavailable` | `a_node_without_the_route_is_reported_as_unsupported` (→ `TelemetryUnsupported`); `a_node_without_a_provider_reports_disabled` (→ `state: disabled`); `an_unreachable_provider_is_reported_as_unavailable` (→ `state: unavailable`, `last_error: connection`) | `crates/eggwork-server/src/lib.rs` tests |
| D. Execution eligibility stays with status/capability/preflight and server-local enforcement | `telemetry_does_not_change_execution_semantics` — an execution still runs to `Succeeded` and creates its marker on a telemetry-enabled node, and the node advertises no telemetry feature | `crates/eggwork-server/src/lib.rs` tests |
| D. Never replace `active_executions` with a telemetry guess; no cluster queue or worker selection | `NodeStatus` is unchanged in shape and meaning; `node_status` was not modified; no new state was added to the server | `crates/eggwork-core/src/lib.rs`, `crates/eggwork-server/src/lib.rs` |
| D. Advertise a static protocol feature only if necessary; do not present provider readiness as an execution capability | **No feature was added.** `static_capability_features` is unchanged, and the installed-binary `status` receipt lists the same features as before with no telemetry entry. | `crates/eggwork-server/src/lib.rs`; installed-binary probe |
| E. Update API, operations and client documentation | `architecture/protocol.md` (the frozen contract), `architecture/server-node.md`, `architecture/client-transport.md`, `docs/operations.md`, `examples/eggworkd.example.json` | as above |
| E. Add a minimal operator command only if needed | **None was added.** The `host-telemetry` doctor check reports whether the provider is configured, and the mTLS route is the live diagnostic surface. No new telemetry control plane exists. | `crates/eggwork-server/src/operations.rs` |

## Verification actually performed

All commands were run from the repository root on the tree `5ccb1e6` produced
(the closure record itself is a later commit).

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo check --locked --workspace` | pass |
| `cargo test --locked --workspace --all-targets` | pass for every suite, **except** one pre-existing `eggwork-sandbox-helper` `ETXTBSY` flake that also reproduces on the unmodified tree — see "Unresolved findings" finding 4. The affected suite passes when the failure does not trigger (it passed in the run immediately before this one) |
| `python3 scripts/check_execution_ownership.py` | pass — "execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)"; "negative scanner self-test passed; crate dependency direction passed" |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | pass — 52 tests, OK |
| `git diff --check` | clean |

Focused runs:

- `cargo test --locked -p eggwork-core` — 28 passed (7 of them the new `NodeTelemetry` contract tests).
- `cargo test --locked -p eggwork-server --lib telemetry_route` — 6 passed.
- `cargo test --locked -p eggwork-server --lib telemetry_client_validation` — 8 passed.
- `cargo test --locked -p eggwork-server --test host_telemetry` — 12 passed (M001, unchanged).
- `cargo test --locked --workspace --all-targets` — `eggwork-server` lib 128 passed, `eggwork-core` 28, `eggwork-client` 9, `eggwork-runner` 18, `release_contract` 36, `host_telemetry` 12, `capability_probe` 2, and `landlock_runner` 15 in the clean run (its one intermittent failure is finding 4 below).

### Installed-release receipt (the no-Gregg smoke test)

The opt-in installed-release qualification was run against a locally built
`eggworkd` and `eggwork-sandbox-helper`, with a node configuration that has
**no `host_telemetry` block** — that is, the default.

```
EGGWORK_QUALIFY_ROOT=<root> \
  cargo test --locked -p eggwork-server --features qualification \
  --test installed_qualification qualification_fixture -- --ignored
  → test qualification_fixture ... ok

EGGWORK_QUALIFY_DAEMON=<root>/bin/eggworkd \
EGGWORK_QUALIFY_CONFIG=<root>/state/node.json \
EGGWORK_QUALIFY_TLS_DIR=<root>/state/tls \
EGGWORK_QUALIFY_PLATFORM=linux \
EGGWORK_QUALIFY_RECEIPT=<root>/receipt.json \
  cargo test --locked -p eggwork-server --features qualification \
  --test installed_qualification \
  installed_release_admits_execution_and_refuses_unsupported_isolation -- --ignored
  → test installed_release_admits_execution_and_refuses_unsupported_isolation ... ok
```

The receipt records, against the installed binary and with **no telemetry
provider configured**: the doctor check `host telemetry is disabled; the node
reports no host measurements`; `configuration` valid; the sandbox helper
runtime-probed as `isolation.landlock.workspace-rw.v1` plus
`resources.cgroups-v2.{memory,cpu,pids}`; an execution reaching `Succeeded`
with exit code 0; required isolation `applied`; and an outside-workspace
write denied.

### Installed-binary route probe

The same installed binary was then driven over real mTLS with the fixture's
client certificate:

```text
# granted principal, no provider configured
GET /v1/node/telemetry
→ HTTP 200
  {"schema_version":1,"node_id":"qualification-node","source":"gregg",
   "state":"disabled","age_ms":null,"last_error":null,"observation":null}

# same binary, principal WITHOUT the "telemetry" grant
GET /v1/node/telemetry
→ HTTP 403
  {"schema_version":1,"code":"forbidden","message":"operation is not authorized"}

# same principal, same instant
GET /v1/status
→ HTTP 200  (node status, unchanged, no telemetry entry in the feature list)

# no client certificate at all
GET /v1/node/telemetry
→ the TLS handshake is refused ("certificate required"); HTTP is never reached
```

That is the whole authorization story on a real binary: the route exists, it
is reachable only over mTLS with a known principal, it is refused with a
typed 403 for a principal that lacks the narrow grant, and the same
principal's ordinary status read is unaffected.

## Platform and feature qualification matrix actually exercised

| Surface | Exercised | Basis |
|---|---|---|
| Authorized mTLS read of a live observation | yes | `an_authorized_principal_reads_the_cached_observation`; installed-binary probe (200 with `state: disabled`) |
| Unauthorized principal (known, not granted) | yes | 403, installed-binary probe and unit test |
| Unknown principal (valid certificate, no grant entry) | yes | 401, `an_unauthorized_principal_cannot_read_the_observation` |
| No client certificate | yes | the TLS handshake is refused; the route is never reached over HTTP |
| Node without a provider | yes | `state: disabled`, unit test and installed-binary probe |
| Node whose provider is unreachable | yes | `state: unavailable`, `last_error: connection` |
| Node that predates the contract | yes | 404 → `TelemetryUnsupported` |
| Remote read does not poll the provider | yes | 16 remote reads, provider request count unchanged |
| Malformed / oversized / unknown-version / unknown-source / self-contradictory / out-of-range response | yes | six focused client tests |
| Node-id mismatch | yes | `telemetry_for` refuses a different node id |
| Installed binary, no Gregg configured | yes | qualification receipt above |
| Execution semantics unaffected | yes | `telemetry_does_not_change_execution_semantics`; the qualification receipt's `Succeeded` execution |
| macOS / Windows hosts | **no** | not exercised. The route, the contract, and the client method are host-independent, but no hosted evidence exists. See "Unresolved findings". |

Nothing above is claimed on the strength of compilation alone: every "yes"
row corresponds to an executed test or an executed probe.

## Security review

- **Trust boundary.** It terminates at Eggwork's mTLS, exactly as ADR-0006
  requires. The Gregg endpoint is never reachable from a remote caller: the
  route is a cache read, the response carries no provider address, and the
  provider's own listener is bound to the node's loopback interface by the
  M001 endpoint rule. No new listener was added and no unauthenticated
  entry point was added.
- **Authorization.** The `telemetry` grant is separate from `status`, so no
  existing grant silently widened. A refused read returns the node's
  existing typed 403 with a fixed message; the unit test asserts the
  rendered error contains no measurement field names, and the installed
  probe shows the fixed body.
- **Information disclosure.** The response carries only host-wide
  aggregates: no provider address, mount path, device or interface
  identity, hostname, per-execution measurement, or raw provider JSON. The
  node's system identity never leaves the node (M001's translation drops it).
- **Trust in the measurement.** A measurement is advisory. It does not
  qualify, withhold, or change an execution capability; `NodeStatus` and
  `NodeCapabilities` are untouched, and the node advertises no telemetry
  feature. A hostile or spoofed provider can therefore produce a wrong
  *measurement*, and nothing more.
- **Resource bounds.** The response is a small bounded document; the client
  caps it at 64 KiB, on top of the transport's own 16 MiB cap. The route
  performs no I/O of its own, so it cannot be used to amplify work against
  the provider.
- **Client-side validation.** A node is a fixed target, and a response
  naming a different node is refused by `telemetry_for`. The client refuses
  an unknown schema version, an unknown source, a self-contradictory
  `state`/`observation` pair, an out-of-range or non-representable number,
  and a body that is not the expected document. A `404` is reported as
  "unsupported", not as a malformed response, so a legacy node is not
  mistaken for a broken one.
- **Redaction.** The new `ClientError::TelemetryUnsupported` carries no
  payload and the hand-written `Debug` prints it as a unit variant, so
  nothing node-controlled reaches a log line through it.

## Compatibility and migration review

- The route is additive. A node that gains it does not change any existing
  response: `NodeStatus`, `NodeCapabilities`, the execution protocol, the
  lease semantics, and the error model are all unchanged, and the
  installed-binary `status` receipt is byte-comparable with a pre-change
  node apart from nothing.
- A principal is not granted telemetry until an operator adds `"telemetry"`
  to its grant list. The `operation` name table gained one entry; every
  other grant name keeps its meaning. An existing configuration file is
  still valid and still loads, and a principal without the new grant simply
  cannot use the new route.
- The client is additive: two new methods and one new error variant, with
  no change to any existing method's behavior.
- No database migration. Nothing about the telemetry read is persisted.
- A client talking to a node that predates the contract gets a typed
  `TelemetryUnsupported`, not a transport failure, so a mixed-version
  deployment degrades cleanly.
- Rollback is "remove the `telemetry` grant", or "remove the route's
  authorization" — there is no state to unwind.

## Documentation status

| Document | State |
|---|---|
| `architecture/protocol.md` | The frozen wire contract is written out in full: every field, the `state`/`observation` table, the measurement-group forms, and an explicit list of what the contract does not carry. |
| `architecture/server-node.md` | New section for the read route: the narrow grant and why it is separate from `status`, the cache-only read, and the 404/403 distinction. |
| `architecture/client-transport.md` | The two client methods, the `TelemetryUnsupported` variant in the error table, and the telemetry-specific refusals. |
| `docs/operations.md` | Operator-facing: the `telemetry` grant in a configuration example, the route, the state table, and the statement that the feature does not change `status` or `capabilities`. |
| `examples/eggworkd.example.json` | The `telemetry` operation name added to the grant list so the grant is discoverable. |

## Unresolved findings

1. **Low — no hosted macOS/Windows evidence.** The route, the contract, and
   the client method are host-independent, and the provider's OS-conditional
   measurements are covered on the fixture host (the `swap` capability is
   false there, which is the interesting case and is covered). No hosted
   run on macOS or Windows was performed, so no platform claim is made
   beyond "host-independent code path, exercised on Linux".
2. **Low — the installed-release receipt is a locally built binary, not a
   published release.** The qualification harness was run against
   `target/debug/eggworkd` at a local installation root, and the receipt
   therefore qualifies *this tree's* binary rather than a tagged release
   artifact. The suite is designed to qualify an installed release and the
   same command line with a published binary would produce a release
   receipt; that is a release-lineage action reserved for the maintainer, in
   the same way Operations M004's installed-release receipt is.
3. **Informational — the provider address appears in `eggworkd config
   print`.** Unchanged from M001 and deliberate: it is the operator's own
   non-secret configuration. It is absent from the telemetry response, from
   every diagnostic line, from every category, and from every metric.
4. **Pre-existing, unrelated — `crates/eggwork-sandbox-helper` `ETXTBSY`
   flake.** The intermittent `Os { code: 26, kind: ExecutableFileBusy }`
   failure seen during the M001 batch recurred in this milestone's runs: it
   hit the full parallel workspace run on a different test than before, and
   it also reproduced in a single-package run of that suite. It is **not**
   caused by this work: the sandbox-helper crate is untouched by `5ccb1e6`,
   and the M001 batch reproduced the same failure on the **unmodified** tree.
   The cause was already measured and dispositioned in
   `plans/closure/operations-distribution/004a-status.md`: a host-level
   kernel window on this machine, reproducible with any copy mechanism and
   with a plain `std::process::Command` spawn outside the runner entirely,
   with no writer holding the inode and with immediate re-spawns succeeding.
   It is neither a fixture defect nor a production defect. It is recorded
   here for honesty, not as a M002 defect, and it is the reason this
   milestone's "full workspace run" line above is qualified.

## Registry and roadmap disposition

- `plans/subsystems/optional-gregg-host-telemetry-roadmap.md` — M002 moves
  from *ready* to **closed**; the roadmap status line, the dependency graph,
  and the milestone list are updated. The subsystem's "Completion" condition
  is now met.
- `plans/registry.md` — the Optional Gregg host telemetry workstream row, the
  dependency-ready plan table, the closed-plans table, and the planned /
  blocked work table are updated. The CodeGG worker-telemetry consumer is
  now unblocked: the interface it needs is frozen and closed.
- `plans/implementation/optional-gregg-host-telemetry/002-authenticated-node-telemetry.md`
  — status `closed`, with a pointer to this record.

**The CodeGG worker telemetry consumer is unblocked.** The plan's condition
was: "Provide a frozen example wire payload, error taxonomy, exact operation
authorization, client method, freshness semantics, reference client test and
a true authenticated integration receipt." All seven are provided above.

## Final disposition

**M002 — closed.** Acceptance criteria are satisfied and the closure evidence
is recorded above. The subsystem roadmap's completion condition — both
milestones closed with code/test receipts and a verified default-off
no-Gregg execution — is met, and the downstream CodeGG consumer can depend
on the frozen, versioned authenticated response.
