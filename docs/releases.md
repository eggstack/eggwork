# Releases and installation

## Who owns what

Producing a release and consuming one are separate authorities. Eggwork sits on
the consuming side of a boundary it does not own.

```text
Eggpack producer -> ReleaseManifest/assets -> human/release channel -> Eggup/Eggwork local deployment
```

1. **Eggpack** builds the release: target and artifact naming, builds,
   qualification, checksum sidecars, the `ReleaseManifest`, bootstrap installers,
   and the generated release workflow.
2. **The release channel** is a published GitHub draft turned public by a human.
   Eggwork never publishes, never creates or moves a tag, and never resolves
   "latest".
3. **Eggup** performs the local verified installation transaction, ownership
   verification, backup/rollback, and service-manager adapters.
4. **Eggwork** owns node configuration, service lifecycle, drain/update/restart
   policy, helper trust, and the installation root.

A fifth, smaller authority is easy to misplace: `.cargo/config.toml` is
Eggwork's own build policy (see [Windows reproducibility](#reproducible-windows-release-binaries)).
Eggpack cannot remove nondeterminism the product's own link flags introduce, and
it is right to fail closed when two builds of one source revision disagree.

Within this repository:

- `release/eggpack/` is the checked-in producer configuration. It is **static**:
  no release tag, no source SHA, and no artifact digest is committed. Release
  identity resolves at run time.
- `.github/workflows/release.yml` is **generated** from that configuration by
  `eggpack ci generate`. Never edit it by hand; `eggpack ci check` in CI fails
  on any drift.
- `install.sh` and `install.ps1` in a release are exact-release **first-install**
  scripts. They install the binaries, never overwrite, never register a
  service, never elevate privileges, and never select a release. Service setup
  afterwards is explicit operator policy through `eggworkd service …`; updates
  are Eggup-owned and are **not** routed through the bootstrap script.
- The staging job prepares a **draft** release only. It creates no tag, moves
  none, publishes nothing, and refuses to clobber differing assets. Publishing is
  a maintainer action.

## Published targets

`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`.

Linux releases ship the daemon **and** its Landlock sandbox helper as one bundle
built from one source revision, so the pair can never drift. macOS and Windows
ship the daemon alone — Landlock is Linux-only.

## Installing a published release

```bash
# after a draft release exists for tag vX.Y.Z
curl -fsSLO https://github.com/eggstack/eggwork/releases/download/vX.Y.Z/install.sh
less install.sh   # read it before running it
sh install.sh /opt/eggwork/bin
```

Check the staged artifacts before installing them:

```bash
sha256sum -c eggwork-vX.Y.Z-<target>.sha256
```

SHA-256 is **integrity evidence, not authenticity**: it proves the bytes match
the staged release, not who published them. Eggup still performs its own
ownership and verification checks on the local side.

Then configure and run the node — see [quickstart.md](quickstart.md).

## Reproducible Windows release binaries

`.cargo/config.toml` scopes one deterministic MSVC link policy to
`x86_64-pc-windows-msvc` only:

- `/BREPRO` — `link.exe` derives the PE COFF timestamp deterministically instead
  of stamping wall-clock time;
- `/DEBUG:NONE` — no CodeView/PDB debug directory is emitted, because a freshly
  generated PDB contributes a per-build RSDS GUID.

Without both flags one fixed source revision produced two different Windows
binaries, and Eggpack correctly refused to replace the differing same-name asset.
The deliberate tradeoff is that release Windows binaries carry **no PDB**; triage
of a shipped build uses a local rebuild from the same revision. Linux and macOS
are untouched.

**Never set `RUSTFLAGS` or `CARGO_*RUSTFLAGS`** — in any environment, including
CI. Cargo would then use the environment *instead of* the target-scoped policy,
and Windows release binaries stop being reproducible.

`.github/workflows/windows-reproducibility.yml` proves the property natively
before any release tag exists: it builds the release Windows daemon twice in
independent target directories on the pinned toolchain, requires byte-identical
SHA-256, and requires no CodeView record in either image
(`scripts/verify_windows_reproducibility.py`).

## Qualifying an installation

`scripts/qualify_release.py` is the product-owned operational qualification
harness. It retrieves the exact bytes of one staged release through the
authenticated GitHub API, verifies every artifact against that release's own
`release-manifest.json` and `.sha256` sidecars, then drives the *installed*
`eggworkd` through the operator commands a human would use.

It never rebuilds a release artifact, never stages or publishes, never mutates a
tag, and invokes the product through an argv list with no shell.

```bash
export RELEASE_QUALIFICATION_TOKEN=...        # a scoped token: a staged candidate is a draft
python3 scripts/qualify_release.py install  --release-tag v0.1.5 --installation-root /opt/eggwork

# the bounded TLS fixture and node configuration come from rcgen, not an external
# OpenSSL/LibreSSL, so the identities are identical on every host
EGGWORK_QUALIFY_ROOT=/opt/eggwork cargo test -p eggwork-server --features qualification \
  --test installed_qualification -- --ignored --exact qualification_fixture

python3 scripts/qualify_release.py installer --release-tag v0.1.5 --installation-root /opt/eggwork
python3 scripts/qualify_release.py service  --installation-root /opt/eggwork
python3 scripts/qualify_release.py update   --release-tag v0.1.5 --release-id v0.1.5 \
  --prior-release-tag v0.1.4 --installation-root /opt/eggwork
```

Installed execution is qualified separately by
`crates/eggwork-server/tests/installed_qualification.rs` — see
[quickstart.md](quickstart.md#7-prove-an-execution-is-actually-confined) for a
locally runnable version of that check.

`.github/workflows/operational-qualification.yml` runs both against a named
release on hosted Linux, macOS, and Windows runners and uploads the bounded
receipts.

## Release identity, and why it is enforced

One release is one coherent identity across three places:

```text
[workspace.package].version  <->  every pin Cargo.lock records for this repository  <->  the git tag
```

Two of those three numbers are owned by the repository, and one is not:

- **`[workspace.package].version` is the build assertion.** It is what the
  binaries report (`eggworkd version`), it is inherited by every crate through
  `version.workspace = true`, and it is read once — by
  `scripts/release_candidate_probe.py`, which every release script uses.
- **The git tag is the release gate.** It says which release this is, and it is
  the only thing a consumer sees.

They may differ in exactly one respect: the tag may carry a `v` prefix, so
`v0.1.6` and `0.1.6` name the same release.

Two releases failed on this, and both were caught late:

- **`v0.1.3`** — the version was bumped without regenerating `Cargo.lock`, so
  every `--locked` release build failed closed. CI had run clippy and test
  *without* `--locked` and therefore passed the gate meant to prevent exactly
  this.
- **`v0.1.4`** — the version bump was missed entirely. The tag said `0.1.4`, the
  staged binaries reported `0.1.3`, and hosted qualification failed every
  install stage with `installed-daemon-version: daemon reports '0.1.3', expected
  0.1.4`.

That second failure was a success: it is the version-coherence check working.
Never weaken a guard to make a release go green.

Before creating a tag, run the local guard:

```bash
python3 scripts/check_release_tag.py v0.1.6
```

**This is a step you perform, not a step CI performs.** Whoever cuts the tag
runs the guard, on the checkout that is about to be tagged, *before* the tag
exists. Nothing in this repository runs it for you: the generated release
workflow is producer authority and cannot be hand-edited to add the step, and CI
never knows the future tag.

What CI *does* run, on every push and every pull request, is the half that
needs no tag:

```bash
python3 scripts/check_release_tag.py --coherence
```

That asserts the repository's own two numbers agree — the manifest's version
and the versions `Cargo.lock` pins for this repository's packages — which is
the condition that broke the `v0.1.3` release attempt. The guard's whole
logic is also exercised against fixtures by the repository's own test suite, so
it is a checked instrument rather than a trusted one.

If the guard fails, the remediation is in its own output: bump `Cargo.toml`,
regenerate the lock, rerun the gates, then tag.

## Design reference

[architecture/release-qualification.md](../architecture/release-qualification.md) (the full
failure history) · [architecture/distribution.md](../architecture/distribution.md) ·
[architecture/ci-guardrails.md](../architecture/ci-guardrails.md) ·
[CONTRIBUTING.md](../CONTRIBUTING.md) (cutting a release)
