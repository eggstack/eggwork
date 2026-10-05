---
name: pre-submit-gate
description: Run the mandatory pre-submit verification gate for eggwork before any commit or PR. Use when finishing a code change, before committing, or when asked "is this ready to submit?". Covers the exact command order, what each guard actually catches, and the historical failure each one exists to prevent.
---

# Pre-submit gate

Run the full gate before every commit. Six of the seven commands are cheap; the
one that costs real time is the workspace test suite, so start it first and read
results while it runs.

```bash
cargo test --locked --workspace --all-targets          # slow — start first
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

## `--locked` is load-bearing on clippy and test

`--locked` is **not** optional on `clippy` and `test`. It is only advisory on
`build` and `check`, which is why those two keep working without it.

The reason is a real release, not a theoretical one. Commit `d5213f7` bumped
`[workspace.package].version` to `0.1.3` without regenerating `Cargo.lock`. Plain
`cargo build` locally *just rewrites the lock and succeeds*. Plain `cargo clippy`
passes. Plain `cargo test` passes. The whole tree is green everywhere — and then
the release workflow, which runs `cargo +1.89.0 build --release --locked`, fails
closed on **all five targets** after preflight and resolve have already spent
their time. `--locked` converts a release-time discovery into a push-time one.

The version bump itself is now half-guarded in CI:
`the_release_version_is_one_coherent_workspace_identity` in
`crates/eggwork-server/tests/release_contract.rs` requires `Cargo.lock` to pin
all five workspace members to `[workspace.package].version`. The other half —
tag ↔ version agreement — cannot live in CI, because CI never knows the future
tag. See the `cut-a-release` skill.

## What each guard actually catches

| Command | Catches | Does **not** catch |
|---|---|---|
| `check_execution_ownership.py` | `Command`/`subprocess`/`duct` spawns outside the two approved owner crates; crate dependency direction | Direct `systemctl`/`launchctl`/SCM invocation — see below |
| `--prove-negative-exit` | Proves the scanner *detects* a synthetic forbidden spawn | **Not** a second full guard pass: it returns before `scan_sources()`/`check_dependencies()` |
| `cargo fmt --check` | Formatting drift (no `rustfmt.toml`, so default rustfmt) | — |
| `cargo clippy … -D warnings` | Lints; `unsafe_code = "deny"` and `clippy::all = "warn"` are workspace-level | — |
| `unittest discover` | The 52 Python harness tests backing `qualify_release.py`, `release_candidate_probe.py`, `verify_windows_reproducibility.py` | The Rust side |
| `git diff --check` | Whitespace errors and conflict markers | — |

## Two limits worth knowing before you trust a green gate

**1. The ownership guard does not police service managers.** Invariant 2 (no
direct `systemctl`/`launchctl`/SCM/`crontab`) has *no* dedicated Python guard.
The one automated check is the Rust test `no_direct_service_manager_invocation_exists`
in `crates/eggwork-server/src/deployment.rs`, and it scans only three files:
`src/deployment.rs`, `src/operations.rs`, `src/bin/eggworkd.rs`. It asserts those
files contain no `"systemctl"` / `"launchctl"` / `"sc.exe"` / `"crontab"` string
literal. **This is your responsibility, not the gate's** — see below.

**2. `--prove-negative-exit` is narrower than its name.** In that mode the script
runs its self-test and then proves the negative exit, but returns before running
`scan_sources()` or `check_dependencies()`. It proves the *scanner* detects a
forbidden spawn, not that the full guard path fails correctly. The plain
invocation is the one that does the real work; run both.

## The `systemctl` rule, stated precisely

The invariant is about **service lifecycle**, not about process spawning. Those
are different things and conflating them makes the rule wrong.

- **Forbidden:** Eggwork managing the *Eggwork node service* by shelling out.
  That lifecycle goes through Eggup adapters
  (`eggup_service::ServiceManager`, `eggup_service::SystemdManager`).
- **Permitted and in use:** `eggwork-runner` invoking `systemd-run --scope` and
  `systemctl show`/`stop` for **per-execution transient cgroup units**. These are
  resource enforcement, owned by the crate that owns process lifecycle, and they
  are documented in `architecture/runner-execution.md` §Resource setup.

If you add a new `systemctl`-shaped call, decide which of those two you are
writing, and make the code and the doc agree. Runner code is subject to the
ownership allowlist; server code is not allowed to spawn at all.

## Scope you must check yourself

The guard globs `crates/*/src/**/*.rs` only. `tests/`, `examples/`, `build.rs`,
and any new top-level crate directory are **unscanned**, and the allowlist is
keyed by crate name rather than path. If you add a crate, a `build.rs`, or a
`tests/` binary that spawns a process, the gate will not tell you.

## Reference

- [AGENTS.md](../../AGENTS.md) — invariants and commands
- [architecture/ci-guardrails.md](../../architecture/ci-guardrails.md) — why each guard exists
- [CONTRIBUTING.md](../../CONTRIBUTING.md) — the canonical pre-submit list
