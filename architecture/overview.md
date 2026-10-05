# Architecture overview

Eggwork is a **fixed-target execution fabric**. A caller selects exactly one node, submits a bounded execution request, observes machine-readable lifecycle events, and receives terminal state plus declared artifacts.

Eggwork is deliberately **not a scheduler**. Global queues, worker selection, priorities, fairness, workflow DAGs, and semantic retry all remain caller-owned. That boundary is the product's defining constraint and is what makes Eggwork usable as an execution backend for orchestrators such as CodeGG without competing with their policy. See [ADR-0001](../plans/adrs/ADR-0001-fixed-target-scheduler-free-execution.md).

This page is the **bird's-eye view**: what each module owns, how they compose, and where to go for a full read. Each component links to a dedicated deep-dive file. For task-shaped agent guidance, see [`.skills/`](../.skills/README.md) — those skills route here rather than restating anything here.

---

## Crate map

```text
                         eggwork-core
                        (no internal deps)
                               |
        +----------------------+----------------------+
        |                      |                      |
 eggwork-runner          eggwork-client        eggwork-sandbox-helper
 (process lifecycle)     (fixed-target client)  (bin: Landlock applier)
        |                                              ^
        |                                              | argv only
        +---------------- eggwork-server --------------+
                    (node service, admission,
                     stores, operations,
                     deployment)
```

| Crate | Role | Depends on | Size |
|---|---|---|---|
| `eggwork-core` | Protocol-neutral bounded domain types: identities, request/result contracts, validation, events | — | 1,671 LOC |
| `eggwork-runner` | The single owner of finite process lifecycle; resource/isolation setup abstraction | core | 2,336 LOC |
| `eggwork-sandbox-helper` | Standalone binary that applies Landlock confinement, verifies the cgroup resource limits, then spawns and supervises the target | — | 401 LOC |
| `eggwork-client` | Explicit single-node client; all HTTP via Eggfetch, optional egress routing | core | 880 LOC |
| `eggwork-server` | Node service, mTLS admission, control plane, stores, operator surface, deployment | core, runner | 14,104 LOC |

The dependency graph is intentionally **one-way**. `eggwork-core` and `eggwork-sandbox-helper` have no production dependency on any other workspace crate. `eggwork-client` depends only on `eggwork-core`. `eggwork-server` depends on `eggwork-core` and `eggwork-runner` only; its dependency on `eggwork-client` is a `dev-dependency` used by the qualification harness. Direction is machine-checked — see [CI and guardrails](ci-guardrails.md).

---

## Components

### 1. `eggwork-core` — the domain contract

Protocol-neutral, bounded, and transport-free. Newtype identities (`NodeId`, `ExecutionId`, `PrincipalId`, `WorkspaceId`, `ArtifactId`), the `ExecutionSpec` request shape, `ExecutionResult`/`ExecutionEvent` outcomes, `NodeCapabilities`, and every protocol limit constant. Two canonical digest functions — `request_digest` and `request_digest_with_workspace` — are version-prefixed and stable against JSON map ordering because core request fields are ordered structs.

This crate is the only shared vocabulary. Everything else agrees on it, and it agrees on nothing in particular about wire format.

→ [Deep dive: core domain](core-domain.md) · Context: [domain model](domain.md), [protocol layering](protocol.md)

### 2. `eggwork-runner` — the one process lifecycle owner

The only component permitted to create OS child processes. `LocalProcessRunner` takes a `RunnerRequest` built via `RunnerRequest::from_spec` (validated fields are private, so callers cannot bypass protocol bounds), executes argv inside a caller-authorized root, clears and rebuilds the environment from a conservative baseline, writes bounded stdin, and drains stdout/stderr concurrently with bounded head/tail capture.

`ExecutionSetup` is the platform-isolation seam. `TrustedLandlockSetup` and `NoExecutionSetup` are the two implementations. `verify_trusted_helper` defines the Linux helper trust boundary once, and every caller — capability advertisement, `doctor`, admission — delegates to that single function so a node never claims isolation its enforcement path would refuse.

Cancellation, timeout, overflow, and leader-exit-with-descendants all signal the process group and force-kill after a grace period. Non-Unix hosts return an explicit unsupported-platform error.

→ [Deep dive: runner execution](runner-execution.md) · Context: [execution ownership](execution-ownership.md)

### 3. `eggwork-sandbox-helper` — the isolation applier

A standalone binary with no workspace-crate dependencies at all. It is not linked into the daemon. It reads a validated `LaunchSpec` from a private file, verifies that the caller's cgroup already enforces the requested resource limits, applies Landlock filesystem restrictions and `no_new_privs`, then **spawns and waits on** the target — it does not `exec`. It must stay alive after the target finishes so it can read post-mortem cgroup counters and report the real resource outcome back to the runner over a `UnixStream` status channel. Confinement is unaffected: the Landlock ruleset is inherited across the fork and preserved across the target's own `execve`.

It creates the target process *only* for this reason — that is the single documented exception to the process-ownership rule. It sets no `RLIMIT_*` itself; resource enforcement is the runner's cgroup, and the helper's job is to *verify* that cgroup actually bounds what was requested.

Linux-only machinery is `#[cfg]`-gated so the crate compiles everywhere; a non-Linux host gets a stub that answers `--version` and otherwise exits 125.

→ [Deep dive: sandbox helper](sandbox-helper.md)

### 4. `eggwork-client` — the explicit fixed-target client

`NodeClient` targets one node explicitly. `ExecutionStream` delivers lifecycle events. All HTTP goes through `eggfetch-core`; the optional `eggress-route` feature adds an in-process outbound route adapter **without changing the target identity**, which preserves the fixed-target guarantee.

→ [Deep dive: client transport](client-transport.md) · Context: [protocol layering](protocol.md)

### 5. `eggwork-server` — the node

The largest surface, split into seven library modules plus the `eggworkd` binary:

- **`lib.rs` (6,280 LOC)** — node configuration, `NodeServer`, the mTLS-authenticated HTTP service, operation dispatch, local admission, lease enforcement, and the full execution lifecycle (`execute`, `monitor_lease`, `run_execution`, `publish`, `publish_terminal`). Authorization is two traits: `PeerPrincipalResolver` (verified TLS leaf → `NodePrincipal`, resolved by SHA-256 fingerprint) and `Authorizer` (operation grant, with a resource-aware `authorize_request` hook for scoped policies).
- **`store.rs`** — the durable SQLite execution and event journal, including idempotency reservation, lease records, and paginated event reads.
- **`blob.rs`** — node-owned content-addressed blob storage with quota enforcement and GC.
- **`workspace.rs`** — validated, execution-owned workspace materialization from manifests.
- **`artifact.rs`** — declared output capture, artifact metadata, retention, and bounded cleanup.
- **`operations.rs` + `bin/eggworkd.rs`** — the JSON-only operator surface: config, `doctor`, drain, execution listing, storage summary, GC, inspection, and service/deployment commands.
- **`deployment.rs`** — consumer deployment and service lifecycle backed by Eggup.

→ Deep dives: [server node](server-node.md) · [storage journal](storage-journal.md) · [data plane](data-plane.md) · [operations CLI](operations-cli.md) · [deployment lifecycle](deployment-lifecycle.md)

### 6. Release, distribution, and qualification

Release production belongs to **Eggpack**, a separate authority. `release/eggpack/` holds static, identity-free producer configuration; `.github/workflows/release.yml` is *generated* from it and never hand-edited. Release consumption and local installation belong to **Eggup** plus `deployment.rs`. Publication is always a human action.

The qualification harness (`scripts/qualify_release.py`, `crates/eggwork-server/tests/installed_qualification.rs`, `release_contract.rs`) verifies exact bytes against a release's own manifest and drives the *installed* binary through real operator commands.

→ Deep dives: [release & qualification](release-qualification.md) · [deployment lifecycle](deployment-lifecycle.md) · Context: [distribution](distribution.md)

### 7. CI, tooling, and guardrails

Architectural invariants are enforced mechanically, not by convention: an execution-ownership checker that scans for forbidden process creation, proves its own negative exit, and validates crate dependency direction; `cargo clippy -D warnings` with workspace-level `unsafe_code = "deny"`; a `--locked` build discipline that exists because a version bump without a lockfile update cost a full release attempt; and a release-drift job that reinstalls the pinned Eggpack revision and fails on any workflow drift.

→ [Deep dive: CI & guardrails](ci-guardrails.md) · Context: [execution ownership](execution-ownership.md)

---

## How a request flows

```text
caller
  │  1. NodeClient (fixed target, explicit node id)
  │     TLS: client cert verified against the node's required client CA
  ▼
eggwork-server
  │  2. dispatch(): resolve leaf fingerprint -> NodePrincipal
  │  3. Authorizer: is this principal granted Execute on this resource?
  │  4. admission: drain state, max active count, declared requirements
  │             vs. NodeCapabilities -> ExecutionAccepted or a typed rejection
  │  5. store.reserve(): idempotency by canonical request digest
  │  6. workspace materialize (manifest -> files, path-validated)
  ▼
eggwork-runner (LocalProcessRunner)
  │  7. ExecutionSetup: verify_trusted_helper -> Landlock + rlimit wrapper
  ▼
eggwork-sandbox-helper
  │  8. verify cgroup limits, apply Landlock + no_new_privs,
  │     spawn and supervise the target, report resource outcome
  ▼
target process
  │  9. events published back; lease monitored; terminal result recorded
  │ 10. artifact capture for declared outputs -> retention/GC eligible
  ▼
caller receives ExecutionResult + declared artifacts
```

Every arrow is a place where the design **fails closed** rather than degrading: unknown isolation capability, untrusted helper, missing grant, persistent drain, or lease expiry all produce an explicit typed error or rejection instead of a weaker execution.

The one deliberate exception is worth stating precisely, because it is easy to over-read. If a `Required` sandbox profile or `Required` resource dimension cannot be enforced, the runner kills the helper and fails the execution with `RunnerError::RequiredSetupUnavailable`. If the caller only asked for an *optional* or *best-effort* requirement and the helper cannot apply it, the execution still runs — unsandboxed — and the result records `SandboxOutcome::NotApplied` / `ResourceSetupOutcome::NotApplied` with the reason. Degradation happens only where the caller never demanded the guarantee, and it is always reported rather than silent.

---

## Invariants that span components

These are the load-bearing rules. Each is mechanically enforced somewhere, not merely documented.

1. **Process ownership is singular.** Only `eggwork-runner` creates OS children. `eggwork-server`, `eggwork-client`, and `eggwork-core` must not. The one exception is the helper, which spawns the target solely to confine it and then supervises it long enough to report the resource outcome; plus a bounded `--version` version-coherence probe that owns no lifecycle. Enforced by `scripts/check_execution_ownership.py`.
2. **No direct service-manager invocation of the node service.** Eggwork never manages its own service by shelling out; all *node service* lifecycle flows through Eggup adapters. The one sanctioned exception is `eggwork-runner`'s use of `systemd-run --scope` and `systemctl show`/`stop` for per-execution transient cgroup units — that is resource enforcement owned by the process-lifecycle crate, not service lifecycle, and it is documented in [runner-execution.md](runner-execution.md). No dedicated mechanical guard covers this (see divergence 6).
3. **Fixed target.** The client names its target explicitly. No queue, no worker selection, no "latest" release resolution, no implicit placement.
4. **Isolation claims are earned, never assumed.** A node advertises filesystem isolation exactly when `verify_trusted_helper` accepts the installed helper. Advisory availability and enforcement agree by construction because they call the same function.
5. **Fail closed on uncertainty — for guarantees the caller actually demanded.** Untrusted helper, version skew, missing previous-generation digest proof, unsupported service backend, and Windows SCM registration are all refusals, not fallbacks to a weaker mode. A `Required` isolation or resource dimension that cannot be enforced fails the execution. Only explicitly optional or best-effort requirements degrade, and they degrade with a recorded `NotApplied` reason rather than silently.
6. **Identity-free release configuration.** No tag, source SHA, artifact digest, or size is committed under `release/eggpack/`. Release identity resolves at run time.
7. **Generated release workflow.** `.github/workflows/release.yml` is rendered by `eggpack ci generate`; CI fails on drift. There is no second hand-maintained release matrix.
8. **No published state.** Config, database, workspace, blob, and artifact state belong to the installation root and never ship in a release artifact.

---

## Divergences and open questions

The deep dives surfaced a set of places where the code, the documentation, and the stated invariants do not line up exactly. They are collected here because they are **cross-cutting** — each one is invisible from any single module — and because they are the natural entry points for targeted review. Every item links to the deep dive that established it with a source citation.

None of these are presented as bugs already judged to be bugs. They are the places where a reviewer should decide whether the code, the doc, or the invariant should move.

### Where the implementation differs from the prose

| # | Observation | Where |
|---|---|---|
| 1 | The sandbox helper **does not `execve`**. It `spawn`s the target and `wait`s, because it must stay alive to read post-mortem cgroup counters and report the real resource outcome to the runner. Confinement still holds — the Landlock ruleset is inherited across the fork and survives the target's own `execve`. | [sandbox-helper.md](sandbox-helper.md) |
| 2 | The helper sets **no `RLIMIT_*`**. `verify_resource_limits` *verifies* that the runner's cgroup already bounds `memory.max`, `memory.swap.max`, `pids.max`, and `cpu.max`. Resource enforcement lives in the runner, not the helper. | [sandbox-helper.md](sandbox-helper.md) |
| 3 | `--windows-start-type` is documented in the README and passed by `scripts/qualify_release.py`, but **no code path parses it**. This is currently harmless: the Windows arm of `product_service_manager` discards `args` and refuses before reaching a manager. It becomes live work when Windows becomes a service host. | [operations-cli.md](operations-cli.md) |

### Where an invariant is weaker in practice than the wording suggests

| # | Observation | Where |
|---|---|---|
| 4 | `GrantAuthorizer` implements only `Authorizer::authorize`, **not** the resource-aware `authorize_request` hook. Resource-scoped authorization is therefore inert on the real deployment path; ownership is enforced only in the store, by `principal_id` comparison. | [operations-cli.md](operations-cli.md) · [server-node.md](server-node.md) |
| 5 | `deployment::check_helper_compatibility_with_timeout` checks the helper **file** only, while `eggwork_runner::verify_trusted_helper` also walks every ancestor directory. The deployment trust path is the weaker subset plus version coherence. | [deployment-lifecycle.md](deployment-lifecycle.md) · [runner-execution.md](runner-execution.md) |
| 6 | Invariant 2 (no direct node-service manager invocation) has **no dedicated mechanical guard**. The only automated check is the Rust test `no_direct_service_manager_invocation_exists` in `deployment.rs`, and it scans just three files (`src/deployment.rs`, `src/operations.rs`, `src/bin/eggworkd.rs`) for manager *string literals*. `eggwork-runner` is outside that scope entirely. | [ci-guardrails.md](ci-guardrails.md) |
| 7 | The ownership guard globs only `crates/*/src/**/*.rs`. `tests/`, `examples/`, and `build.rs` are unscanned, and the allowlist is keyed by crate name rather than path. | [ci-guardrails.md](ci-guardrails.md) |
| 8 | `--prove-negative-exit` is **narrower than its name**: in that mode the guard runs its self-test and then proves the negative exit, but never runs `scan_sources()` or `check_dependencies()`. It proves the scanner detects, not that the full guard path fails correctly. | [ci-guardrails.md](ci-guardrails.md) |
| 9 | `blob_download` takes **no reference lease**, unlike `artifact_download`. A concurrent GC can unlink the file mid-transfer; this is safe only because Unix keeps the open descriptor alive. | [data-plane.md](data-plane.md) |
| 10 | Workspace materialization opens destinations with `create_new(true)` on a plain `Path::join` — no `O_NOFOLLOW`, no `openat2` — whereas artifact capture uses a descriptor-relative `NOFOLLOW` chain. Workspace is the weaker symlink path in the codebase, mitigated by the fresh `0700` staging root. | [data-plane.md](data-plane.md) |

### Correctness and robustness questions in the store

| # | Observation | Where |
|---|---|---|
| 11 | **No schema version or migration path.** Only `CREATE TABLE IF NOT EXISTS`; no `PRAGMA user_version`. `ExecutionSnapshot.schema_version` lives inside the JSON and is never validated on read. | [storage-journal.md](storage-journal.md) |
| 12 | The store **does not enforce the state machine.** `commit_event` accepts any non-terminal → non-terminal transition; all legality lives in `publish`. The terminal/non-terminal sets are spelled out in four places with nothing keeping them in sync. | [storage-journal.md](storage-journal.md) |
| 13 | **Unchecked `u64 → i64` narrowing.** `ExecutionGeneration::new` rejects only `0`, yet the store casts `generation.get() as i64` in six places. Same class of issue for `after_sequence as i64` in `load_page`, guarded only at the HTTP route. | [storage-journal.md](storage-journal.md) |
| 14 | `load_page` has **no `LIMIT`** and is not a consistent read — three separate autocommit statements, bounded only indirectly. | [storage-journal.md](storage-journal.md) |
| 15 | `MAX_EXECUTION_IDENTITIES` counts `(id, generation)` rows rather than identities and **never evicts**, so it is a permanent 503 after the cap. | [storage-journal.md](storage-journal.md) |
| 16 | **TOCTOU between admission and launch**: the execution permit is acquired before the idempotency `reserve` await, and the runner task is spawned after it. A crash in that window leaves a durable reservation for an execution that never starts. | [server-node.md](server-node.md) |
| 17 | Blob GC can **race `retain`**: GC holds the write lock while `retain` holds only the metadata mutex, so a reference can commit between `remove_file` and the guarded delete. Recovered by the `NOT EXISTS` guard and `remove_stale_parts`, but the window is untested. | [data-plane.md](data-plane.md) |
| 18 | Artifact GC does **not release blob references**; reclaiming bytes requires artifact GC then blob GC, guaranteed only by call order in `collect_garbage`. | [data-plane.md](data-plane.md) |

### Client-side transport hygiene

| # | Observation | Where |
|---|---|---|
| 19 | `events()` puts the lease id — documented in core as **bearer authority** and redacted from `Debug` — into a **URL query string**, while `cancel`/`renew` put it in a JSON body. | [client-transport.md](client-transport.md) |
| 20 | `decode_json` calls `response.bytes()` with **no size cap**. The node bounds its own request bodies, but the client bounds only the NDJSON line buffer, leaving the JSON paths open to memory exhaustion from a hostile node. | [client-transport.md](client-transport.md) |
| 21 | Redirect and retry safety is an **accident of the dependency profile**: the workspace's `standard-http1` omits eggfetch's `redirects` and `logical-retry` features, so 3xx is returned unfollowed. Bumping to the `http1` alias would silently enable both and no test would fail. | [client-transport.md](client-transport.md) |

### Positive notes worth keeping

- Authorization strictly **precedes** resource resolution in `dispatch`, and id parsing touches only the path, so a denied caller cannot distinguish an existing resource from an absent one.
- Lease expiry **rewrites a successful runner result** in place to `Interrupted`/`LeaseExpired`; success is never claimed without authority.
- The store uses **literal SQL with `?n` bindings only** — no dynamic SQL construction, so no injection surface.
- Windows service management fails closed with a detailed, reasoned error **before** any mutation, and the typed adapter is deliberately retained for the future milestone.
- The no-clobber rule on staging assets is what proved, during the `v0.1.4` refusal, that **nothing was replaced** when a candidate failed qualification.

---

## Document index

**Foundations**

| Document | What it covers |
|---|---|
| [domain.md](domain.md) | Normative domain vocabulary and identity invariants |
| [protocol.md](protocol.md) | Protocol layering intent and pre-wire status |
| [execution-ownership.md](execution-ownership.md) | Who may create processes, and the one exception |
| [distribution.md](distribution.md) | Producer/consumer/release-channel authority split |

**Component deep dives**

| Document | Component |
|---|---|
| [core-domain.md](core-domain.md) | `eggwork-core` — bounded domain types and validation |
| [runner-execution.md](runner-execution.md) | `eggwork-runner` — process lifecycle and isolation setup |
| [sandbox-helper.md](sandbox-helper.md) | `eggwork-sandbox-helper` — Landlock and rlimit applier |
| [client-transport.md](client-transport.md) | `eggwork-client` — fixed-target client and transport |
| [server-node.md](server-node.md) | `eggwork-server` lib — node service, admission, control plane |
| [storage-journal.md](storage-journal.md) | `store.rs` — durable execution/event journal and leases |
| [data-plane.md](data-plane.md) | `blob.rs`, `workspace.rs`, `artifact.rs` — content-addressed data |
| [operations-cli.md](operations-cli.md) | `operations.rs` + `eggworkd.rs` — operator surface |
| [deployment-lifecycle.md](deployment-lifecycle.md) | `deployment.rs` — Eggup-backed install, update, service |
| [release-qualification.md](release-qualification.md) | Release config, qualification harness, evidence |
| [ci-guardrails.md](ci-guardrails.md) | CI workflows, ownership checker, lints, discipline |

**Normative sources elsewhere in the repository**

- [`docs/`](../docs/README.md) — user/operator-facing documentation, starting at `docs/quickstart.md`. This directory is the internal design record; where they differ, this one wins.
- [`plans/000-long-term-specification.md`](../plans/000-long-term-specification.md) — canonical product/architecture specification
- [`plans/registry.md`](../plans/registry.md) — current ready/blocked work
- [`plans/adrs/`](../plans/adrs/) — decision records
- [`plans/closure/`](../plans/closure/) — implementation and verification evidence
