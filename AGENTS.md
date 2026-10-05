# AGENTS.md

Rust-native, fixed-target remote execution fabric: a caller selects one node, submits a bounded execution request, observes machine-readable lifecycle events, and receives terminal state plus declared artifacts. It is deliberately **not a scheduler** — queues, worker selection, priorities/fairness, DAGs, and semantic retry stay caller-owned.

## Setup commands

- Toolchain: pinned by `rust-toolchain.toml` (1.89.0, rustfmt + clippy, minimal profile)
- Install deps: `cargo build --locked`
- Test:         `cargo test --locked --workspace --all-targets`
- Lint:         `cargo fmt --all -- --check` and `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- Check:        `cargo check --workspace`
- Ownership guard: `python3 scripts/check_execution_ownership.py` (also `--prove-negative-exit`)
- Harness tests: `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release`
- Run the node: `cargo run -p eggwork-server --bin eggworkd -- run --config <node.json>`

`--locked` is load-bearing on `clippy` and `test`: a version bump that skips `Cargo.lock` passes both and then fails every release build.

## Project layout

- `crates/eggwork-core/` — protocol-neutral bounded domain types; no internal deps
- `crates/eggwork-runner/` — the single owner of OS process lifecycle + isolation setup seam
- `crates/eggwork-sandbox-helper/` — standalone Landlock/cgroup binary; **no** workspace-crate deps
- `crates/eggwork-client/` — explicit single-node client (feature: `eggress-route`)
- `crates/eggwork-server/` — node service, mTLS control plane, stores, `eggworkd` operator CLI, deployment
- `architecture/` — deep-dive design docs; start at `architecture/overview.md`
- `plans/` — planning system (`plans/README.md`, `003-planning-process.md`, `registry.md` = current handoff)
- `scripts/` — Python guardrails and qualification harness
- `release/eggpack/` — static, identity-free producer config (Eggpack owns releases)
- `examples/` — `eggworkd.example.json` node config shape
- `tests/release/` — Python unit tests for the release/qualification scripts
- `.skills/` — task-shaped agent guidance (see [below](#skills))

## Skills

Read the relevant skill before starting the matching kind of work. Each is
derived guidance that routes to a normative document; where a skill and
`AGENTS.md` or `architecture/` disagree, the deep dive wins and the skill is
what gets fixed.

| Skill | Use it when |
|---|---|
| `.skills/pre-submit-gate/` | Finishing a change or asked "is this ready to submit?" — the exact gate, what each guard catches, and the two limits of the ownership guard |
| `.skills/cut-a-release/` | Bumping a version, cutting/tagging a release, or diagnosing a failed release run |
| `.skills/architecture-routing/` | Before reading source — which deep dive is normative, which code owns which boundary, and whether a suspected bug is an already-documented divergence |
| `.skills/isolation-and-trust/` | Touching Landlock/cgroups, helper trust, capability advertisement, or admission |

## Architecture index

`architecture/overview.md` is the bird's-eye view and carries a **divergences
table** — read it before diagnosing anything, because several entries are known
and deliberate. Deep dives:

| Area | Document |
|---|---|
| Vocabulary, identities, invariants | [domain.md](architecture/domain.md) |
| Wire-level status, layering intent | [protocol.md](architecture/protocol.md) |
| Who may create processes | [execution-ownership.md](architecture/execution-ownership.md) |
| Producer/consumer/release-channel split | [distribution.md](architecture/distribution.md) |
| `eggwork-core` contracts, limits, digests | [core-domain.md](architecture/core-domain.md) |
| `eggwork-runner` launch, env, cancellation, setup seam | [runner-execution.md](architecture/runner-execution.md) |
| Landlock, cgroup verification, helper supervision | [sandbox-helper.md](architecture/sandbox-helper.md) |
| `eggwork-client` mTLS, egress routing, retries | [client-transport.md](architecture/client-transport.md) |
| Admission, authorization, leases, lifecycle | [server-node.md](architecture/server-node.md) |
| SQLite journal, state machine, pagination | [storage-journal.md](architecture/storage-journal.md) |
| Blobs, workspaces, artifacts, GC | [data-plane.md](architecture/data-plane.md) |
| Operator verbs, `doctor`, drain, GC | [operations-cli.md](architecture/operations-cli.md) |
| Eggup install/update/rollback, service policy | [deployment-lifecycle.md](architecture/deployment-lifecycle.md) |
| Release harness, hosted qualification, failures | [release-qualification.md](architecture/release-qualification.md) |
| CI jobs, ownership guard, `--locked` discipline | [ci-guardrails.md](architecture/ci-guardrails.md) |

## Hard invariants (mechanically enforced — do not weaken)

1. **Process ownership is singular.** Only `eggwork-runner` and `eggwork-sandbox-helper` may create child processes; `eggwork-core`, `eggwork-client`, `eggwork-server` must not. Also banned outright: `command`, `duct`, `process-wrap`, `portable-pty`, `subprocess`. The allowlist lives in `scripts/check_execution_ownership.py`; adding an owner requires an architecture review.
2. **No direct service-manager invocation for the *node service*.** Eggwork never manages its own service by shelling out — the node service lifecycle goes through Eggup adapters. **Exception, and it is not a violation:** `eggwork-runner` invokes `systemd-run --scope` and `systemctl show`/`stop` for *per-execution transient cgroup units*, which is resource enforcement owned by the process-lifecycle crate, not service lifecycle. A new `systemctl`-shaped call must pick a side and match the code to it. (No dedicated Python guard; the only automated check, `no_direct_service_manager_invocation_exists`, scans three `eggwork-server` files. `eggwork-runner` is not covered — discipline is on you.)
3. **Crate dependencies stay one-way.** core/helper → nothing; client → core; server → core + runner (`eggwork-client` is dev-only for the server).
4. **Isolation claims are earned.** Advertise filesystem isolation only when `verify_trusted_helper` accepts the installed helper; the same function backs capability advertisement, `doctor`, and admission.
5. **Fail closed** on untrusted helper, version skew, missing prior-generation digest, unsupported service backend. Degrade only when the caller never demanded a guarantee, and always record `NotApplied`.
6. **No release identity in `release/eggpack/`** — no tag, source SHA, artifact digest, or size.
7. **`.github/workflows/release.yml` is generated** by `eggpack ci generate`. Never hand-edit; `eggpack ci check` fails on drift.
8. **Never set `RUSTFLAGS`/`CARGO_*RUSTFLAGS`** — the env would silently replace the target-scoped Windows link policy in `.cargo/config.toml` (`/BREPRO`, `/DEBUG:NONE`) that makes MSVC release binaries reproducible.
9. **Windows service management fails closed.** `deployment::windows_scm_manager` is retained but refuses before any mutation; daemon has no SCM dispatcher.

## Code style

- Rust 2024 edition; no `rustfmt.toml`, so default rustfmt. `unsafe_code = "deny"` and `clippy::all = "warn"` at workspace level; CI denies warnings.
- Errors via `thiserror` with explicit typed variants; no panics on untrusted input.
- Construct validated state behind private fields (`RunnerRequest::from_spec`) so callers cannot bypass protocol bounds.
- Digests are version-prefixed and stable against JSON map ordering; core request fields stay ordered structs.
- Comments explain *why* an invariant exists, referencing the failure it prevents — match the existing density in `architecture/`.
- `#[cfg]`-gate Linux-only machinery (Landlock, cgroups) with an explicit fail-closed stub so all five release targets build.

## Testing instructions

- Rust: `cargo test --locked --workspace --all-targets`. Integration tests live in `crates/*/tests/`.
- Release/qualification scripts: the `unittest discover` command above.
- `--test installed_qualification` is **ignored** and opt-in: run with `-p eggwork-server --features qualification` plus `EGGWORK_QUALIFY_DAEMON` / `EGGWORK_QUALIFY_CONFIG` / `EGGWORK_QUALIFY_TLS_DIR` (or `EGGWORK_QUALIFY_ROOT`) and `--ignored`.
- Adding a release target or artifact form also requires updating `crates/eggwork-server/tests/release_contract.rs`.
- Run the full `CONTRIBUTING.md` pre-submit list plus `git diff --check` before submitting.

## PR & commit conventions

- Branch from `main`; never push to it directly.
- Pre-submit gate: ownership guard, fmt, clippy, workspace tests, workspace check, release-harness tests.
- Planning changes follow `plans/003-planning-process.md`; do not edit `plans/000`-`002` as part of ordinary implementation.
- The next piece of ready work is in `plans/registry.md` (currently Operations M007, the Windows service host).

## Security

- Report suspected vulnerabilities privately to the maintainers (see `SECURITY.md`); no public issue with exploit detail.
- Never commit secrets, node private keys, or published state (config, SQLite journal, workspaces, blobs, artifacts) — the latter belong to the installation root and never ship in a release artifact.
- Node trust requires a verified mTLS client CA and explicit per-principal grants; `config print` redacts private-key paths and certificate fingerprints.
- SHA-256 sidecars are integrity evidence, not authenticity.
