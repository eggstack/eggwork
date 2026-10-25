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
| `eggwork-core` | Protocol-neutral bounded domain types: identities, request/result contracts, validation, events | — | see the size script |
| `eggwork-runner` | The single owner of finite process lifecycle; resource/isolation setup abstraction | core | see the size script |
| `eggwork-sandbox-helper` | Standalone binary that applies Landlock confinement, verifies the cgroup resource limits, then spawns and supervises the target | — | see the size script |
| `eggwork-client` | Explicit single-node client; all HTTP via Eggfetch, optional egress routing | core | see the size script |
| `eggwork-server` | Node service, mTLS admission, control plane, stores, operator surface, deployment | core, runner | see the size script |

The size column is **not a number in this document**, and that is deliberate. A
checked-in line count is a fact about the tree that rots on the next edit and
is wrong the moment it is written — which is the same defect the citation gate
removes from the prose above. The figures are printed on demand instead:

```
python3 scripts/component_sizes.py
```

The script counts every line of every `.rs` file a crate owns, split into the
crate's `src/` (the product) and `tests/` (the evidence), and sums to a
workspace total so a reader can check the arithmetic. It is stdlib-only and
deterministic: the same tree always produces the same output, and the output
depends on nothing but the files on disk.

The **script** is the durable artifact here, not any particular run of it. A
snapshot of its output belongs in a closure record, where it is evidence about
one commit, rather than in this document, where it would be a second owner of
a number the tree already owns.

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

- **`crates/eggwork-server/src/lib.rs`** — node configuration, `NodeServer`, the mTLS-authenticated HTTP service, operation dispatch, local admission, lease enforcement, and the full execution lifecycle (`execute`, `monitor_lease`, `run_execution`, `publish`, `publish_terminal`). Authorization is two traits: `PeerPrincipalResolver` (verified TLS leaf → `NodePrincipal`, resolved by SHA-256 fingerprint) and `Authorizer` (operation grant, with a resource-aware `authorize_request` hook for scoped policies).
- **`store.rs`** — the durable SQLite execution and event journal, including idempotency reservation, lease records, and paginated event reads.
- **`blob.rs`** — node-owned content-addressed blob storage with quota enforcement and GC.
- **`workspace.rs`** — validated, execution-owned workspace materialization from manifests.
- **`artifact.rs`** — declared output capture, artifact metadata, retention, and bounded cleanup.
- **`operations.rs` + `crates/eggwork-server/src/bin/eggworkd.rs`** — the JSON-only operator surface: config, `doctor`, drain, execution listing, storage summary, GC, inspection, and service/deployment commands.
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

**Re-verification, 2026-10-25 (M008b).** Every row was re-checked against the tree rather than assumed. Row **3** was corrected (the flag is documented in `docs/deployment.md`, not in a README) and row **20** was closed (the client's transport already caps every decoded body). Every other row was checked and left alone: each observation is still true of the code as it stands, and the already-closed rows **4**, **17**, **22** and **23** still hold, residual boundaries included. Row **24** was preserved verbatim. No row was renumbered; a closed row is closed in place, and a new row takes the next free number at the end of the document.

### Where the implementation differs from the prose

| # | Observation | Where |
|---|---|---|
| 1 | The sandbox helper **does not `execve`**. It `spawn`s the target and `wait`s, because it must stay alive to read post-mortem cgroup counters and report the real resource outcome to the runner. Confinement still holds — the Landlock ruleset is inherited across the fork and survives the target's own `execve`. | [sandbox-helper.md](sandbox-helper.md) |
| 2 | The helper sets **no `RLIMIT_*`**. `verify_resource_limits` *verifies* that the runner's cgroup already bounds `memory.max`, `memory.swap.max`, `pids.max`, and `cpu.max`. Resource enforcement lives in the runner, not the helper. | [sandbox-helper.md](sandbox-helper.md) |
| 3 | `--windows-start-type` is documented in `docs/deployment.md` and passed by `scripts/qualify_release.py`, but **no code path parses it**. This is currently harmless: the Windows arm of `product_service_manager` discards `args` and refuses before reaching a manager. It becomes live work when Windows becomes a service host. | [operations-cli.md](operations-cli.md) |

### Where an invariant is weaker in practice than the wording suggests

| # | Observation | Where |
|---|---|---|
| 4 | ~~`GrantAuthorizer` implements only `Authorizer::authorize`, **not** the resource-aware `authorize_request` hook. Resource-scoped authorization is therefore inert on the real deployment path; ownership is enforced only in the store, by `principal_id` comparison.~~ **Closed 2026-10-25 by [ADR-0007](../plans/adrs/ADR-0007-resource-scoped-authorization.md) (Option A).** `GrantAuthorizer` now implements `authorize_request`: a grant may carry a bounded, optional list of resource scopes naming an operation, a resource kind, and a typed identifier, and the decision narrows the operation grant against that configuration-derived set. The scope is per `(operation, kind)`, so a scope for `Execute`/`Execution` does not narrow `Execute`/`Workspace`. The store's `principal_id` fences remain authoritative for ownership, so a scope naming another principal's execution, workspace, or artifact is still refused there. **Residual boundary:** a grant with no scope is still operation-wide, so a broad `BlobRead` still reaches any known digest — blob storage is content-addressed and shared, so a blob has no owner to fence it, and a blob scope is an explicit digest list the operator must configure. | [operations-cli.md](operations-cli.md) · [server-node.md](server-node.md) · [threat-model.md](../plans/security/threat-model.md) |
| 5 | `deployment::check_helper_compatibility_with_timeout` checks the helper **file** only, while `eggwork_runner::verify_trusted_helper` also walks every ancestor directory. The deployment trust path is the weaker subset plus version coherence. | [deployment-lifecycle.md](deployment-lifecycle.md) · [runner-execution.md](runner-execution.md) |
| 6 | Invariant 2 (no direct node-service manager invocation) has **no dedicated mechanical guard**. The only automated check is the Rust test `no_direct_service_manager_invocation_exists` in `deployment.rs`, and it scans just three files (`crates/eggwork-server/src/deployment.rs`, `crates/eggwork-server/src/operations.rs`, `crates/eggwork-server/src/bin/eggworkd.rs`) for manager *string literals*. `eggwork-runner` is outside that scope entirely. | [ci-guardrails.md](ci-guardrails.md) |
| 7 | The ownership guard globs only `crates/*/src/**/*.rs`. `tests/`, `examples/`, and a Cargo build script are unscanned, and the allowlist is keyed by crate name rather than path. | [ci-guardrails.md](ci-guardrails.md) |
| 8 | `--prove-negative-exit` is **narrower than its name**: in that mode the guard runs its self-test and then proves the negative exit, but never runs `scan_sources()` or `check_dependencies()`. It proves the scanner detects, not that the full guard path fails correctly. | [ci-guardrails.md](ci-guardrails.md) |
| 9 | `blob_download` takes **no reference lease**, unlike `artifact_download`. A concurrent GC can unlink the file mid-transfer; this is safe only because Unix keeps the open descriptor alive. | [data-plane.md](data-plane.md) |
| 10 | Workspace materialization opens destinations with `create_new(true)` on a plain `Path::join` — no `O_NOFOLLOW`, no `openat2` — whereas artifact capture uses a descriptor-relative `NOFOLLOW` chain. Workspace is the weaker symlink path in the codebase, mitigated by the fresh `0700` staging root. | [data-plane.md](data-plane.md) |

### Correctness and robustness questions in the store

| # | Observation | Where |
|---|---|---|
| 11 | **No schema version or migration path.** Only `CREATE TABLE IF NOT EXISTS`; no `PRAGMA user_version`. `ExecutionSnapshot.schema_version` lives inside the JSON and is never validated on read. | [storage-journal.md](storage-journal.md) |
| 12 | The store **does not enforce the state machine.** `commit_event` accepts any non-terminal → non-terminal transition; all legality lives in `publish`. The terminal/non-terminal *classification* is now spelled out in one place — `eggwork-core`'s `ExecutionState::is_terminal` / `is_non_terminal` — and the server's queries and predicates derive from it, so the sets can no longer drift apart; the *legality* of a transition is still enforced nowhere but `publish`. | [storage-journal.md](storage-journal.md) |
| 13 | **Unchecked `u64 → i64` narrowing.** `ExecutionGeneration::new` rejects only `0`, yet the store casts `generation.get() as i64` in **eleven** places (the deep dive previously said six). Same class of issue for `after_sequence as i64` in `load_page`, guarded only at the HTTP route. | [storage-journal.md](storage-journal.md) |
| 14 | `load_page` has **no `LIMIT`** and is not a consistent read — three separate autocommit statements, bounded only indirectly. | [storage-journal.md](storage-journal.md) |
| 15 | `MAX_EXECUTION_IDENTITIES` counts `(id, generation)` rows rather than identities and **never evicts**, so it is a permanent 503 after the cap. | [storage-journal.md](storage-journal.md) |
| 16 | **TOCTOU between admission and launch**: the execution permit is acquired before the idempotency `reserve` await, and the runner task is spawned after it. A crash in that window leaves a durable reservation for an execution that never starts. | [server-node.md](server-node.md) |
| 17 | ~~Blob GC can race `retain`.~~ **Closed 2026-10-07.** The entry asserted GC held the write lock while `retain` held "only the metadata mutex", but `garbage_collect` takes *both* — it locks `inner.metadata` at entry and holds it across `commit()`, and `retain` needs that same mutex for its whole body. They are mutually exclusive, so no reference can commit between `remove_file` and the guarded delete. The `NOT EXISTS` guard remains as defence in depth. | [data-plane.md](data-plane.md) |
| 18 | Artifact GC does **not release blob references**; reclaiming bytes requires artifact GC then blob GC, guaranteed only by call order in `collect_garbage`. | [data-plane.md](data-plane.md) |

### Client-side transport hygiene

| # | Observation | Where |
|---|---|---|
| 19 | `events()` puts the lease id — documented in core as **bearer authority** and redacted from `Debug` — into a **URL query string**, while `cancel`/`renew` put it in a JSON body. | [client-transport.md](client-transport.md) |
| 20 | ~~`decode_json` calls `response.bytes()` with **no size cap**. The node bounds its own request bodies, but the client bounds only the NDJSON line buffer, leaving the JSON paths open to memory exhaustion from a hostile node.~~ **Closed 2026-10-25 by the `17ff53f` corrective.** The client's `HttpClient` is built with `.max_decoded_body_size(MAX_BUFFERED_BODY_BYTES)`, so Eggfetch wraps the response body in a limiting stream that refuses to decode more than 16 MiB — on the compressed path and the uncompressed path alike — and `decode_json` reads a body the transport has already bounded. The telemetry route is bounded twice: the transport cap, then a 64 KiB check on the decoded length. **Residual boundary:** the cap is a *transport* bound, not a per-route one, so a hostile node can still make the client buffer up to 16 MiB before the route's own check rejects the reply. The node's own request-body bounds are unchanged. | [client-transport.md](client-transport.md) |
| 21 | Redirect and retry safety is an **accident of the dependency profile**: the workspace's `standard-http1` omits eggfetch's `redirects` and `logical-retry` features, so 3xx is returned unfollowed. Bumping to the `http1` alias would silently enable both and no test would fail. | [client-transport.md](client-transport.md) |

### Platform limitations

This table carries the rows that are about *a platform*, not about a module. It
is separate so the numbering stays monotonic: rows 1-21 are cited elsewhere by
number, and a row inserted into an earlier table would renumber them.

| # | Observation | Where |
|---|---|---|
| 22 | **Declared artifacts are refused, not captured, on a platform that cannot open a file beneath the workspace root safely.** `open_file_beneath` is a descriptor-relative `NOFOLLOW` chain with no non-Unix implementation, so a non-Unix node cannot capture declared outputs. ~~Before the M005a corrective, the finalization set the finalization-failure marker and left the terminal state `Succeeded`, so a caller that declared outputs saw a successful execution with zero artifacts and no error — the declared outputs were silently dropped — and the node advertised `artifact.declared.v1` on every platform.~~ **Closed by the M005a corrective.** The platform's capture-capability fact now reports the limitation, a node that cannot capture does not advertise `artifact.declared.v1`, and an execution that declares outputs there ends in `Failed` with a `Failed` capture disposition and a diagnostic event. The limitation itself remains: the product's top-level claim of *terminal state plus declared artifacts* holds on a Unix platform only. | [data-plane.md](data-plane.md) · [protocol.md](protocol.md) |

### Operator-surface corrections

This table carries the rows the M008a corrective closed. It is separate for
the same reason the platform table is: the rows are numbered monotonically
across the document, rows 1-22 are cited elsewhere by number, and a row
inserted into an earlier table would renumber them.

| # | Observation | Where |
|---|---|---|
| 23 | ~~The operator surface's metric vocabulary had no owner that was the producer.~~ **Closed by the M008a corrective.** `operations.rs` declared a hand-maintained `METRIC_NAMES` and the report copied a stored value *only* when the name was already in its map, so a counter the server incremented but the list did not name was dropped by the report — five of them, all workspace counters. The vocabulary is now a typed `Metric` enumeration declared with the producers in `crates/eggwork-server/src/lib.rs`; the report reads that declaration, and a row whose name the vocabulary does not know is reported under its own name rather than dropped. | [operations-cli.md](operations-cli.md) · [storage-journal.md](storage-journal.md) |
| 24 | ~~One field name, `active_executions`, carried two facts.~~ **Closed by the M008a corrective.** On the wire it is the number of execution permits the node holds; in the operator's own listing it was the number of durable execution rows in a non-terminal state, and nothing in the output said which was which. The wire field keeps its published name and its published meaning — renaming it would break every deployed client that deserializes `NodeStatus` — and the operator surface now names the two facts apart: `execution_permits_held` for the permit count, `non_terminal_executions` for the row count. The old operator-side `active_executions` key is removed rather than aliased, because an alias would keep the ambiguity alive and a silently changed meaning is worse than a missing key. | [operations-cli.md](operations-cli.md) · [server-node.md](server-node.md) |

### Positive notes worth keeping

- Authorization strictly **precedes** resource resolution in `dispatch`, and id parsing touches only the path, so a denied caller cannot distinguish an existing resource from an absent one.
- Lease expiry **rewrites a successful runner result** in place to `Interrupted`/`LeaseExpired`; success is never claimed without authority.
- The store uses **literal SQL with `?n` bindings only** — no dynamic SQL construction, so no injection surface.
- Windows service management **used to** fail closed with a detailed, reasoned error **before** any mutation, and the typed adapter was deliberately retained for a future milestone. The daemon is now a service host, the adapter is the live path, and the failure it prevented — a registration with no service behind it — is what the hosted lifecycle job now proves cannot happen.
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
