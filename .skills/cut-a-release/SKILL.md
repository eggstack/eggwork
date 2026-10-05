---
name: cut-a-release
description: Cut an eggwork release end to end - version bump, lockfile, tag, generated workflow, hosted run, draft, publish. Use when asked to cut/tag/bump/publish a release, or when a release run failed. Encodes the four authority boundaries and the tag-version-lock identity that cost two failed release attempts.
---

# Cutting a release

Eggwork **does not own release production**. Eggpack is the producer authority;
Eggup owns local install and service lifecycle; publication is a human action.
Getting this boundary wrong is how you end up hand-editing a generated file.

```text
Eggpack producer -> ReleaseManifest/assets -> human/release channel -> Eggup/Eggwork local deployment
```

## The identity chain, and why it is the whole job

One release is one coherent identity across three places. They must agree:

```text
[workspace.package].version   <->   every eggwork-* pin in Cargo.lock   <->   the git tag
```

Two real failures came from breaking it, and both were caught late and at
release time:

- **`v0.1.3`** — commit `d5213f7` bumped `Cargo.toml` and forgot the lock. Every
  `--locked` release build failed closed. Fixed by `b31f117`, which also added
  `--locked` to CI's clippy and test steps.
- **`v0.1.4`** — commit `b31f117` missed the version bump *entirely*. The tag
  said `0.1.4`, the staged binaries reported `0.1.3`, and hosted qualification
  failed **every** install stage with
  `installed-daemon-version: daemon reports '0.1.3', expected 0.1.4`.

That second failure is a success: it is the version-coherence check working.
Never weaken a guard to make a release go green.

## Procedure

### 1. Bump and lock, together

```bash
# edit [workspace.package].version in Cargo.toml, then IMMEDIATELY:
cargo update --workspace        # or: cargo generate-lockfile
```

Verify the lock actually moved and matches:

```bash
grep -A1 '^name = "eggwork-' Cargo.lock
```

All five `eggwork-*` packages must show the new version. If they still show the
old one, the bump is incomplete and every release build will fail.

### 2. Run the full pre-submit gate

See the `pre-submit-gate` skill. The `--locked` flag on clippy and test is what
turns a `v0.1.3`-class failure into a push-time failure instead of a release-time
one.

### 3. Guard the tag *before* it exists

```bash
python3 scripts/check_release_tag.py v0.1.6
```

It requires the tag (minus `v`) to equal `[workspace.package].version`, every
`eggwork-*` package in `Cargo.lock` to be pinned there, and a clean tree. On
failure it ends with the remediation: `bump Cargo.toml, regenerate the lock,
rerun the gates, then tag.`

**This check deliberately is not in CI.** The generated release workflow is
producer authority — the `release-drift` job rejects hand edits to it — and CI
never knows the future tag. It runs at tag time, by whoever cuts the tag. Do not
"helpfully" move it into the workflow.

### 4. Land, then tag, then push both

```bash
git checkout main && git pull --ff-only
python3 scripts/check_release_tag.py v0.1.6
git tag v0.1.6
git push origin main && git push origin v0.1.6
```

### 5. Run the Release workflow

Trigger **Release** from `Actions` with `release_tag` set to that exact existing
tag. There is no latest/branch fallback: a run refuses a tag that does not
exist, and every job re-verifies the checked-out `HEAD` against the resolved
release plan.

The run builds and natively qualifies all five required targets, gates on
required evidence, finalizes checksum sidecars and a `ReleaseManifest`, and
stages a **draft**.

### 6. Inspect, then publish — as a human

The staging job creates no tag, moves none, publishes nothing, and refuses to
clobber a differing same-name asset. Publication stays a manual action.

A rerun of an already-staged release re-reconciles the exact remote asset set
and **fails closed** if any digest differs, rather than overwriting. That
no-clobber rule is what proved, during the `v0.1.4` refusal, that nothing was
replaced when a candidate failed qualification.

## Never hand-edit the generated workflow

`.github/workflows/release.yml` (~51 KB) is **generated** by `eggpack ci
generate` from `release/eggpack/`. Hand edits are rejected by `eggpack ci check`
in ordinary CI (`release-drift` job). If you change producer configuration,
regenerate with the **exact pinned tool revision** — not latest `main`:

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

Using a different revision than the one pinned in `github-policy.json` is how
you introduce drift you did not intend.

## Never commit release identity

`release/eggpack/` is static and identity-free by design. No tag, no source SHA,
no artifact digest, no artifact size — release identity resolves at run time. CI
fails the build if that is violated.

## Never set RUSTFLAGS

`.cargo/config.toml` scopes `/BREPRO` and `/DEBUG:NONE` to
`x86_64-pc-windows-msvc` only. Setting `RUSTFLAGS` or `CARGO_*RUSTFLAGS` in any
environment makes Cargo use the environment **instead of** the target-scoped
policy, and Windows release binaries stop being reproducible. The
`no_workflow_or_environment_can_replace_the_target_scoped_rustflags` test
enforces this.

Without both flags, one fixed source revision produced two different Windows
binaries and Eggpack correctly refused to replace the differing same-name asset
(`v0.1.0` incident: same sections, different COFF timestamp, different
CodeView RSDS GUID). The tradeoff is that release Windows binaries carry no PDB;
triage a shipped build by rebuilding the same revision locally.

## Adding a target or artifact form

That also requires updating `crates/eggwork-server/tests/release_contract.rs`
(36 tests, ~1,390 lines), which reconciles the producer configuration against
Eggwork's deployment install unit (`install_unit_matrix` in `deployment.rs`).

## Reference

- [CONTRIBUTING.md](../../CONTRIBUTING.md) — cutting a release
- [docs/releases.md](../../docs/releases.md) — the operator-facing view of the same material
- [architecture/release-qualification.md](../../architecture/release-qualification.md) — the full failure history
- [architecture/ci-guardrails.md](../../architecture/ci-guardrails.md) — `--locked` and the ownership guard
- [architecture/distribution.md](../../architecture/distribution.md) — authority split
