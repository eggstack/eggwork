# Operations M001 Closure — Node Operations Surface

Status: closed

Reviewed implementation commits:

- `1011a2b6401ad54eb8009fcb63da895c0d546a60` — node CLI, configuration, diagnostics, inspection, storage, GC, persistent drain, and metrics.
- `50b87b2c432a4291755172693cab9642cd593203` — recovery metrics and metadata-verified blob inspection.

Implementation plan: `plans/implementation/operations-distribution/001-node-operations-surface.md`.

## Requirement-to-evidence matrix

| Requirement | Evidence | Result |
|---|---|---|
| Config validate/print and deterministic version command | Valid required-mTLS config test, invalid config/TLS test, redacted config test; CLI `version` returned `0.1.0`; CLI `config print --config examples/eggworkd.example.json` redacted private-key path and client certificate fingerprint | Pass |
| Doctor covers config/TLS, bind viability, storage, helper, and runtime resource probes | `operations::doctor` reports fixed named checks, validates EggServe TLS identity/client CA, checks storage roots/free space and listener, and probes configured trusted helper capabilities; invalid configuration returns not-ready without secret material | Pass on Linux host |
| Drain blocks new work without queueing and survives restart | Existing authenticated server test changes the shared drain marker while a process is live and verifies new work is rejected; `operator_drain_persists_across_node_restart` verifies persisted drain and undrain; no queue was added | Pass |
| Execution list/show is bounded and paginated | `execution_listing_is_bounded_and_paginates_over_durable_records`; maximum list page is 200 and retained identity scan is capped at the store bound | Pass |
| Storage/artifact/workspace/blob inspection | CLI inspection reads metadata without following symlinks; blob inspection cross-checks content path and metadata size; operator tests cover symlink-safe storage sizing | Pass |
| Bounded safe GC and live-execution behavior | GC preview uses read-only SQLite connections; preview test proves no partial upload is removed; applying offline GC is blocked by the running-node state lock; authenticated service test invokes the live-store GC path during execution and confirms the execution remains active | Pass |
| Bounded status metrics without ID cardinality | Fixed counter set stores accepted/rejected categories, terminal outcomes, blob/stdout/stderr bytes, event-history resyncs, and cleanup failures. Status combines counters with active/limit, durable states, storage usage, and backend health. `metrics_report_uses_a_fixed_bounded_counter_set` verifies unknown keys are omitted | Pass |

## Verification

- Host: Linux x86_64, kernel `6.8.0-139-generic`; Cargo `1.89.0`.
- `rtk cargo test --workspace --all-targets` — 77 passed across 7 suites.
- `rtk cargo clippy --workspace --all-targets -- -D warnings` — passed.
- `rtk git diff --check` — passed.
- `rtk cargo run --quiet -p eggwork-server --bin eggworkd -- version` — returned JSON version `0.1.0`.
- `rtk cargo run --quiet -p eggwork-server --bin eggworkd -- config print --config examples/eggworkd.example.json` — succeeded with secret-bearing config values redacted.

## Platform, security, and compatibility

The CLI, config loading, required mTLS identity construction, TLS key permissions, systemd resource probe integration, and drain lifecycle were exercised on Linux x86_64. Windows and macOS builds/runtime behavior were not qualified. Service packaging and OS service registration are outside this milestone.

Configuration is version 1 JSON. Relative paths resolve from the config file. TLS requires a server identity, client CA, and explicit principal-to-operation grants; private-key material is never printed. Config/key ownership and permissions are checked. Operator output uses fixed fields and fixed metric labels; raw execution IDs do not become metric labels.

Offline `gc --apply` requires the node to be stopped and takes an exclusive state lock to avoid racing live stores. The live `NodeServer::collect_garbage` API handles bounded cleanup against the daemon's existing stores while executions continue. Doctor treats an occupied bind that accepts TCP as an active listener but cannot identify that listener locally.

No unresolved findings block M001 closure. Platform packaging and native service-manager ownership remain in Operations M002.

## Registry and roadmap disposition

Operations M001 is closed. Operations M002 remains blocked pending Eggup service-manager adapters suitable for safe systemd, launchd, and Windows SCM integration. CodeGG M001 remains dependency-ready but stays sequenced after Operations M002 per the requested order.

## Next-plan dependency review

On 2026-09-23, the published `eggup-core`, `eggup-acquisition`, `eggup-eggfetch`,
and `eggup-service` 0.1.0 interfaces were rechecked against the Operations M002
handoff. The core exposes the verified multi-artifact transaction and the
acquisition crates expose a transport contract/adapter. `eggup-service`
documents `ServiceManager` as manager-neutral and says platform adapters belong
to later milestones; the published implementation provides `TestDoubleManager`
only and performs no real manager calls. See the [eggup-service 0.1.0 API](https://docs.rs/eggup-service/0.1.0/eggup_service/) and
[eggup-core 0.1.0 API](https://docs.rs/eggup-core/0.1.0/eggup_core/).

That leaves no safe Eggup-owned adapter for the service ownership/start/stop
work required by Operations M002. Implementing manager lifecycle in Eggwork
would duplicate the boundary this plan requires Eggup to own, so M002 remains
blocked. No later plan in the requested sequence is started until this
dependency changes.
