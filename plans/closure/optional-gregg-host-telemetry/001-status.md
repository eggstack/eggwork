# Optional Gregg host telemetry — M001 closure

Status: **closed**
Closed: 2026-10-19
Source plan: `plans/implementation/optional-gregg-host-telemetry/001-loopback-gregg-adapter.md`
Subsystem roadmap: `plans/subsystems/optional-gregg-host-telemetry-roadmap.md`
Decision: `plans/adrs/ADR-0006-optional-gregg-host-telemetry.md`
Branch: `planning/optional-gregg-telemetry-m001-m002`
Implementation commit: `ac55985` — *feat(server): default-off loopback Gregg host-telemetry adapter (M001)*
Reviewed head at closure: `ac55985`
Reviewed upstream baseline: `dfa4efd` (planning handoffs), branched from `main` at `17ff53f`

## Executive finding

M001 is implemented and closed. A node may now be configured to read host-wide
measurements from a local telemetry provider, cache them, and keep them
current, with the provider entirely optional and off by default.

The three properties the milestone was written to establish are established by
construction rather than by convention:

1. **The provider endpoint cannot leave the loopback interface.** The endpoint
   is validated once, at config-load time, against a closed rule set, and the
   client is built without redirect and proxy support. A hostname, a non-loopback
   address, a `3xx`, or an inherited `HTTP_PROXY` cannot move the request.
2. **One owner polls.** A single background task performs every provider
   request. Readers consult the cache; they never trigger a request, so a burst
   of callers cannot become a burst of provider requests.
3. **A missing measurement is never zero.** Every measurement group is `Value`,
   `Unsupported`, or `Unknown`, and the provider's own capability flags decide
   which. Nothing is fabricated and nothing is clamped.

The node's execution semantics are untouched. `NodeStatus` and
`NodeCapabilities` keep their existing meaning, admission is unchanged, and
with no `host_telemetry` block the node creates no provider, starts no task,
and claims no measurements.

## What landed

| Area | Change |
|---|---|
| `crates/eggwork-core/src/lib.rs` | Protocol-neutral measurement vocabulary: `TelemetrySource`, `TelemetryProviderState`, `TelemetryErrorCategory`, `TelemetryMetric<T>`, `TelemetryCpu`/`Memory`/`Load`/`Swap`/`Disk`/`DiskIo`/`Network`, `HostTelemetry` and `HostTelemetry::validate()`. No Gregg types; no change to `NodeStatus`/`NodeCapabilities`. |
| `crates/eggwork-server/src/telemetry.rs` (new) | The provider seam (`HostTelemetryProvider`, `ProviderOutcome`, `TelemetryError`), the Gregg adapter (`GreggTelemetryProvider`), endpoint validation (`validate_endpoint`, `names_explicit_port`), the normalization (`normalize`), the cache (`TelemetryCache`), the polling owner (`TelemetryMonitor`), and the refresh loop with bounded backoff. |
| `crates/eggwork-server/src/lib.rs` | `NodeConfig::host_telemetry`, `NodeState::telemetry`, monitor construction in `NodeServer::start`, and monitor shutdown in `NodeServer::shutdown`. |
| `crates/eggwork-server/src/operations.rs` | `OperatorConfig::host_telemetry` (`HostTelemetrySettings`), config-load-time validation, `node_config()` passthrough, and the `host-telemetry` doctor check. |
| `crates/eggwork-server/tests/host_telemetry.rs` (new) | Behavioural tests against a real loopback HTTP provider server. |
| `crates/eggwork-server/Cargo.toml` | `eggfetch-core`, `url`, `async-trait` promoted to runtime dependencies; `gregg-protocol 1.0.15` added. |
| `docs/operations.md`, `docs/quickstart.md` | Operator-facing guidance: the optional config block, the loopback requirement, the freshness/staleness rules, and the statement that the default `eggworkd` runs without a provider. |
| `architecture/server-node.md` | New "Optional host telemetry" deep-dive section, the `NodeConfig` table row, and four new invariant rows. |

## Requirement-to-evidence matrix

| Plan requirement | Evidence | Where |
|---|---|---|
| A. Validated loopback endpoint; refuse userinfo, non-loopback IP, DNS/hostname, redirect, inherited proxy, request-derived URL; fixed path; no auto-discovery or new listener | `a_non_loopback_or_ambiguous_endpoint_is_refused` (23 refused shapes: `localhost`, `example.com`, `10.0.0.1`, `::ffff:127.0.0.1`, `https`, `file`, userinfo, query, fragment, wrong path, missing/zero/out-of-range port); `an_alternative_numeric_spelling_is_canonicalized_to_the_address`; `a_redirect_is_not_followed` (asserts exactly one request) | `crates/eggwork-server/src/telemetry.rs` tests; `crates/eggwork-server/tests/host_telemetry.rs` |
| B. One node-scoped owner; bounded timeouts, body cap, backoff; no spin when the provider is down; no launch-time wait | `one_owner_polls_for_every_reader` (64 concurrent readers, one provider call); `a_stopped_monitor_stops_polling`; `the_backoff_grows_and_is_capped`; `a_provider_that_never_answers_times_out`; `an_oversized_body_is_refused` | as above |
| C. Typed error/freshness semantics: disabled, warming, unavailable, stale, malformed, valid-with-unsupported-field | `a_disabled_monitor_never_polls`; `a_warming_provider_is_reported_as_warming`; `an_unexpected_status_is_reported_as_unavailable`; `a_malformed_body_is_reported_as_unavailable`; `a_stale_observation_is_reported_as_stale`; `a_provider_timestamp_in_the_future_is_refused`; `a_malformed_body_is_refused`; `a_wrong_schema_version_is_refused` | as above |
| D. Normalize only host-wide measurements; never attach raw Gregg JSON | `a_supported_measurement_is_carried_and_an_unsupported_one_is_not`; `an_unsupported_capability_is_not_reported_as_a_missing_value`; `a_supported_but_unreported_measurement_is_unknown`; `drive_and_throughput_data_is_aggregated_without_identifiers` (asserts the encoded observation contains neither the drive name nor the device id) | `crates/eggwork-server/src/telemetry.rs` tests |
| E. Default-off; no Gregg process, port, listener, or dependency when unconfigured | `a_configuration_without_host_telemetry_asks_for_none`; `a_disabled_monitor_never_polls`; `doctor_reports_that_host_telemetry_is_off_by_default`; no subprocess spawn site exists in the new module (ownership guard: approved process owners are unchanged) | `crates/eggwork-server/src/operations.rs` tests; `crates/eggwork-server/tests/host_telemetry.rs` |
| E. Lifecycle and logging: categorized and rate-limited, no arbitrary bodies/paths | `TelemetryError::category()` is a closed enum; the `host-telemetry` doctor check asserts the provider address is absent from the diagnostic line | `crates/eggwork-server/src/operations.rs` tests |
| F. Make the internal response type ready for future mTLS projection | `eggwork_core::HostTelemetry` is provider-neutral and serializable; the M002 projection input is already a stable value type with a `validate()` | `crates/eggwork-core/src/lib.rs` |
| G. Docs: Gregg must be bound to loopback; default `eggworkd` runs without Gregg | "Host telemetry (optional, off by default)" section; quickstart note | `docs/operations.md`, `docs/quickstart.md` |
| G. Source-level footprint and dependency note | See "Dependency and footprint note" below | this record |

## Verification actually performed

All commands were run from the repository root at `ac55985` (the tree the
commit produced; the closure record itself is a later commit).

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo check --locked --workspace` | pass |
| `cargo test --locked --workspace --all-targets` | pass except one **pre-existing** flake, see "Unresolved findings" |
| `python3 scripts/check_execution_ownership.py` | pass — "execution ownership guard passed; approved process owners: eggwork-runner (2 spawn sites), eggwork-sandbox-helper (1 spawn sites)"; "negative scanner self-test passed; crate dependency direction passed" |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | pass — 52 tests, OK |
| `git diff --check` | clean |

Focused test runs:

- `cargo test --locked -p eggwork-core` — 21 passed.
- `cargo test --locked -p eggwork-server --lib telemetry` — 14 passed.
- `cargo test --locked -p eggwork-server --lib operations` — 16 passed.
- `cargo test --locked -p eggwork-server --test host_telemetry` — 12 passed.

Test counts above are the counts observed in the runs recorded here, not
estimates.

## Platform and feature matrix actually exercised

| Surface | Exercised | Basis |
|---|---|---|
| Loopback IPv4 provider (`127.0.0.1`) | yes | integration tests against a real loopback HTTP server |
| Loopback IPv6 provider (`[::1]`) | **no** | endpoint validation accepts it and the URL is canonical; no live IPv6 loopback server was bound in this environment, so only the validation path is proven here |
| Provider warming (503 health envelope) | yes | `a_warming_provider_is_reported_as_warming` |
| Provider down (connection refused) | yes | `an_unreachable_provider_is_reported_as_unavailable` |
| Provider hanging | yes | `a_provider_that_never_answers_times_out` (300 ms total deadline) |
| Provider redirecting | yes | `a_redirect_is_not_followed` |
| Provider over-sized / malformed / wrong schema | yes | three focused tests |
| Default-off node (no provider) | yes | configuration, doctor, and monitor tests |
| macOS / Windows hosts | **no** | not exercised; the code path is host-independent and the provider supplies OS-conditional metrics, but no hosted evidence exists. See "Unresolved findings". |

Nothing above is claimed on the strength of compilation alone: every "yes" row
corresponds to an executed test.

## Security review

- **SSRF / request redirection.** The endpoint must be a loopback IP literal
  with an explicit port and the fixed provider path. A hostname is refused
  outright, so no resolution and therefore no rebinding window exists. The
  client is built without Eggfetch's `redirects` and `proxy` features, so a
  `3xx` is reported as an unexpected status and no ambient proxy is consulted.
  The URL comes only from the operator's config; no request can supply or
  influence it.
- **Local spoofing.** A local process could serve a forged `/v2/status`. The
  consequence is bounded: a spoofed payload is still schema-validated by the
  provider's own rules and by `HostTelemetry::validate()`, and the worst case
  is a wrong *measurement*, which is advisory and never enforcement evidence.
  This is recorded rather than defended, and matches ADR-0006's treatment of
  the local trust boundary.
- **Host-information exposure.** The provider's system identity, drive names,
  device identities, and interface identities are dropped in `normalize`. The
  normalized observation is asserted to contain neither the drive name nor the
  device id. Storage capacity is an aggregate over the reported volumes and
  carries no mount path.
- **Metadata in diagnostics.** Provider failures are a closed category
  enum. No response body, provider URL, or host path enters a category, a log
  line, or a metric. The doctor check does not echo the provider address; the
  operator-facing `config print` does show the configured endpoint, because
  that is the operator's own non-secret configuration and they need to see
  what they configured. The endpoint carries no credentials — userinfo is
  refused at validation.
- **Resource bounds.** Decoded body ≤ 256 KiB, connect deadline 2 s, total
  deadline 5 s, one poll per interval, bounded backoff. A hostile or broken
  provider cannot spin the node or exhaust its memory.
- **Privilege.** The adapter runs in the daemon's own process and reads
  nothing but the provider's HTTP response. It creates no subprocess, opens no
  listener, and claims no capability. The ownership guard's approved process
  owners are unchanged.

## Compatibility and migration review

- The provider is off by default. A node whose config has no `host_telemetry`
  block is byte-for-byte the same node it was: no provider, no task, no
  measurement, no new capability, no change to `NodeStatus`,
  `NodeCapabilities`, admission, leases, or execution semantics.
- The config addition is one optional object with `deny_unknown_fields`, so an
  existing config file still loads and an unknown key inside the new object is
  refused rather than ignored.
- `HostTelemetry` is new vocabulary in `eggwork-core`. It is additive: no
  existing type changed shape, no serialization of an existing type changed,
  and `NodeStatus`/`NodeCapabilities` are untouched. A node without a provider
  never produces one.
- No database migration. The cache is in memory; nothing is persisted.
- Rollback is "remove the `host_telemetry` block", or leave the provider
  running and stop the node. There is no on-disk state to unwind.

## Dependency and footprint note

The plan asked for a measured dependency note, and for `gregg-protocol` to be
preferred only if the dependency impact is reasonable.

- `gregg-protocol 1.0.15` is used directly. It adds **three** packages to
  `Cargo.lock`: `gregg-protocol`, `thiserror 1.0.69`, and `thiserror-impl
  1.0.69` (the provider's crate is on `thiserror` 1.x while this workspace is
  on 2.x). It introduces **no new dependency family** — `serde`, `serde_json`,
  and `thiserror` were already in the graph.
- The alternative was a hand-rolled minimal DTO with equivalent strict
  validation. That would have avoided the three packages but would have
  re-implemented the provider's validation rules in Eggwork, which is exactly
  the drift the plan asked to avoid ("a minimal DTO with equivalent strict
  validation" only if the dependency cost is *material*). The cost here is
  three packages and one duplicated `thiserror` major version, which is not
  material against a rule that "no raw provider JSON, no copied protocol
  logic" is worth more.
- The published `gregg-protocol` 1.0.15 `v2.rs` was compared byte-for-byte
  against the reviewed Gregg working tree and is identical, so the wire types
  this milestone coded against are the shipped ones.
- `eggfetch-core` moved from a dev-dependency to a runtime dependency of
  `eggwork-server`. It was already a workspace dependency of `eggwork-client`
  at the same version and feature set (`standard-http1`, `tls-rustls`), so this
  adds no new package to the graph. `url` and `async-trait` were likewise
  already workspace dependencies.
- The provider client deliberately uses the `standard-http1` feature set
  *without* `redirects` and *without* `proxy`. That is the mechanism behind
  the "no redirect hop, no inherited proxy" property, not an accident of
  configuration.

## Documentation status

| Document | State |
|---|---|
| `docs/operations.md` | Updated: the readiness table now lists eight checks including `host-telemetry`, and a new "Host telemetry (optional, off by default)" section covers the config block, the loopback requirement and exactly which endpoint shapes are refused, the single-owner polling model, the freshness and staleness rules, and the bounded failure categories. |
| `docs/quickstart.md` | Updated: states that the config above has no `host_telemetry` block, so the node creates no provider and reports no host measurements, and links to the operations section. |
| `architecture/server-node.md` | Updated: new "Optional host telemetry" deep-dive section, the `NodeConfig` table row, and four new invariant rows pointing at the tests that pin them. |
| `examples/eggworkd.example.json` | Deliberately unchanged. The committed example is the *default-off* shape; the optional block is documented in `docs/operations.md` so the example does not imply telemetry is on. |

## Unresolved findings

1. **Low — IPv6 loopback provider is validated but not exercised against a live
   server.** `http://[::1]:PORT/v2/status` is accepted and canonicalized, and
   the refusal list covers the IPv4-mapped and link-local forms, but no test
   bound an IPv6 loopback listener in this environment. The validation path is
   proven; the transport path is not. Not a closure blocker: the provider
   contract is the same over either family, and the IPv4 path is fully
   exercised.
2. **Low — no hosted macOS/Windows evidence.** The adapter is host-independent
   and the provider supplies OS-conditional metrics (the `swap` capability is
   false on the fixture host, which is the interesting OS-conditional case and
   *is* covered). No hosted run on macOS or Windows was performed, so no
   platform claim is made beyond "host-independent code path, exercised on
   Linux".
3. **Pre-existing, unrelated — `crates/eggwork-sandbox-helper` `ETXTBSY`
   flake.** `crates/eggwork-sandbox-helper/tests/landlock_runner.rs` fails
   intermittently under the full parallel workspace run with
   `Os { code: 26, kind: ExecutableFileBusy }` from `execve` of the staged
   helper. This is **not** caused by this milestone: the sandbox-helper crate
   is untouched by `ac55985` (`git diff --name-only` lists no sandbox file),
   the test passes 3/3 in isolation on this tree, and the same failure was
   reproduced on the **unmodified** tree at `dfa4efd` (run 1 of 3 clean-tree
   full-suite runs). It is the host-level kernel window already recorded and
   re-dispositioned in `plans/closure/operations-distribution/004a-status.md`.
   It is recorded here for honesty, not as a M001 defect.
4. **Informational — the provider address appears in `eggworkd config print`.**
   This is deliberate (see "Security review"): it is the operator's own
   non-secret configuration, it carries no credentials, and the operator needs
   to see what they configured. It is absent from every diagnostic line,
   category, metric, and log.

## Registry and roadmap disposition

- `plans/subsystems/optional-gregg-host-telemetry-roadmap.md` — M001 moves
  from *ready* to **closed**; the roadmap status line and the dependency graph
  are updated; M002 moves from *blocked on M001 closure* to **ready**.
- `plans/registry.md` — the Optional Gregg host telemetry workstream row,
  the dependency-ready plan table, the closed-plans table, and the planned /
  blocked work table are updated. M002 is unlocked.
- `plans/implementation/optional-gregg-host-telemetry/001-loopback-gregg-adapter.md`
  — status `closed`, with a pointer to this record.
- `plans/implementation/optional-gregg-host-telemetry/002-authenticated-node-telemetry.md`
  — status `ready`; its blocker (M001 closure freezing the bounded normalized
  provider contract) is discharged by `eggwork_core::HostTelemetry` and
  `eggwork_core::TelemetryProviderState`.

**M002 is unblocked.** The interface it relies on exists and is proven: a
protocol-neutral, validated, provider-neutral observation type with a closed
provider-state enum and a closed error-category enum.

## Final disposition

**M001 — closed.** Acceptance criteria are satisfied and the closure evidence
is recorded above. M002 is unlocked and ready for handoff.
