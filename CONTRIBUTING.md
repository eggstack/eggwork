# Contributing

Changes must preserve the scheduler-free fixed-target boundary and crate ownership described in `architecture/`. Run formatting, Clippy, workspace tests, workspace check, and `python3 scripts/check_execution_ownership.py` before submitting Rust changes. Production process creation belongs only to `eggwork-runner`, with the documented sandbox-helper exception. The *node service* lifecycle must reach service managers only through Eggup adapters — `eggwork-runner`'s use of `systemd-run --scope`/`systemctl` for per-execution transient cgroup units is resource enforcement, not service lifecycle, and is the sanctioned exception. Planning changes must follow `plans/003-planning-process.md`.

Task-shaped guidance for common work lives in [`.skills/`](.skills/README.md):
the pre-submit gate, cutting a release, routing into `architecture/`, and
isolation/trust boundaries.

## Pre-submit commands

```bash
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --workspace
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

`--locked` on the clippy and test steps is load-bearing. A workspace version bump
that does not update `Cargo.lock` builds fine locally and passes both without it,
then fails every release build at `cargo build --locked`. That cost a full
release attempt — the `v0.1.3` producer run failed closed on it. `--locked`
converts a release-time discovery into a push-time one.

Note that `--prove-negative-exit` is narrower than its name: it proves the
scanner detects a forbidden spawn, not that the full guard path fails correctly.
The plain invocation above is the one that does the real work; run both.

## Release configuration changes

Eggpack is the producer authority for the release matrix. `release/eggpack/` is
the checked-in configuration and `.github/workflows/release.yml` is generated
from it, so never edit the workflow by hand. After changing anything under
`release/eggpack/`, regenerate and verify with the exact pinned tool revision:

```bash
revision="$(python3 -c "import json,sys;sys.stdout.write(json.load(open('release/eggpack/github-policy.json'))['eggpack_tool']['revision'])")"
cargo install --git https://github.com/eggstack/eggpack --rev "$revision" --locked eggpack-cli

eggpack ci generate \
  --workflow-shape release/eggpack/workflow-shape.json \
  --contract release/eggpack/distribution.toml \
  --github-policy release/eggpack/github-policy.json \
  --output .github/workflows/release.yml

eggpack ci check \
  --workflow-shape release/eggpack/workflow-shape.json \
  --contract release/eggpack/distribution.toml \
  --github-policy release/eggpack/github-policy.json \
  --workflow .github/workflows/release.yml
```

Never commit a release tag, source SHA, artifact digest, or artifact size into
`release/eggpack/`. The static configuration is identity-free by design; CI
fails the build if it is not. Adding a target or changing an artifact form also
requires updating `crates/eggwork-server/tests/release_contract.rs`, which
reconciles the producer configuration against Eggwork's deployment install unit.

## Cutting a release

1. Land and tag the release commit, then push the tag.
2. Run the **Release** workflow from `Actions` with the `release_tag` input set
   to that exact existing tag. There is no latest/branch fallback; a run refuses
   a tag that does not exist and every job re-verifies the checked-out `HEAD`
   against the resolved release plan.
3. The run builds and natively qualifies all five required targets, gates on
   required evidence, finalizes the release with checksum sidecars and a
   `ReleaseManifest`, and stages a **draft** release.
4. Inspect the draft, then publish it. Publication is a human action; the
   workflow never publishes, creates a tag, or clobbers a differing asset.

A rerun of an already-staged release re-reconciles the exact remote asset set
and fails closed if a digest differs, rather than overwriting it.
