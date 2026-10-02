# eggwork

Eggwork is a Rust-native, fixed-target remote execution fabric: a caller selects one node, submits a bounded execution request, observes machine-readable lifecycle events, and receives terminal state plus declared artifacts.

Eggwork is deliberately **not another scheduler**. Global queues, worker selection, priorities/fairness, workflow DAGs, and semantic retry remain caller-owned. This makes Eggwork suitable as an execution backend for systems such as CodeGG without competing with their orchestration policy.

The protocol-neutral core, canonical local finite-process runner, authenticated remote control plane, durable leases, workspace transfer, declared artifact capture, bounded retention/garbage collection, authorization/redaction, Linux sandbox/resource controls, the first node operations surface, Eggup-backed consumer deployment/service lifecycle, and the Eggpack-backed producer release surface are implemented. The CodeGG adapter remains planned work.

## Node operations

`eggworkd` provides a JSON-only operator interface. It reads a versioned JSON config, requires a verified mTLS client CA and explicit per-principal operation grants, and redacts the private-key path and client certificate fingerprints from `config print`.

See [examples/eggworkd.example.json](examples/eggworkd.example.json) for the config shape. Replace its example paths and certificate fingerprint with installation-specific values; keep the private key owned by the node account and readable only by that account.

```text
eggworkd config validate --config /etc/eggwork/node.json
eggworkd doctor --config /etc/eggwork/node.json
eggworkd run --config /etc/eggwork/node.json
eggworkd status --config /etc/eggwork/node.json
eggworkd drain --config /etc/eggwork/node.json
eggworkd undrain --config /etc/eggwork/node.json
eggworkd executions list --config /etc/eggwork/node.json --limit 50 --offset 0
eggworkd executions show --config /etc/eggwork/node.json <execution-id>
eggworkd storage summary --config /etc/eggwork/node.json
eggworkd inspect blob|workspace|artifact <id> --config /etc/eggwork/node.json
eggworkd gc --config /etc/eggwork/node.json --limit 128       # read-only preview
eggworkd gc --config /etc/eggwork/node.json --apply --limit 128
eggworkd deployment status --config /etc/eggwork/node.json
eggworkd service spec --config /etc/eggwork/node.json --executable /opt/eggwork/bin/eggworkd --service-config /etc/eggwork/node.json
eggworkd service status --config /etc/eggwork/node.json --executable /opt/eggwork/bin/eggworkd --service-config /etc/eggwork/node.json
eggworkd service install|start|stop|restart|uninstall --config /etc/eggwork/node.json --executable /opt/eggwork/bin/eggworkd --service-config /etc/eggwork/node.json --unit-path /etc/systemd/system/eggwork-node.service --scope system
eggworkd deployment apply --config /etc/eggwork/node.json --installation-root /opt/eggwork --release v0.1.1 --daemon /var/tmp/eggworkd --helper /var/tmp/eggwork-sandbox-helper --service-config /etc/eggwork/node.json --unit-path /etc/systemd/system/eggwork-node.service --scope system
eggworkd version
```

Consumer deployment and service lifecycle go through Eggup (`eggup-core`/`eggup-service` 0.1.1): the install unit is `bin/eggworkd` plus `bin/eggwork-sandbox-helper` moved in one transaction, service ownership is proven by exact executable plus critical argv/config before any stop/replace/uninstall, and a bounded post-install check runs inside Eggup's lifecycle transaction while rollback evidence remains available (`RollBack` is the default). `RecoveryRequired` preserves Eggup's exact transaction evidence and suppresses automatic restoration of the old service generation. Updates drain first: set persistent drain with `eggworkd drain`, wait for active executions (or pass explicit force policy through the orchestration API), then replace. Service manager mutation must use an explicit product policy: Linux `--unit-path/--scope`, macOS `--plist-path/--launchd-domain/--launchd-target`, or Windows `--windows-start-type`. Adapter availability alone is never a platform support claim: a service backend is only claimed as supported where the native manager was actually mutated on a real host.

`deployment apply` accepts already-local candidates only; it does not fetch a URL, resolve a release, or publish. Linux release updates require both daemon and helper. A replacement can pass `--previous-daemon-sha256` and, on Linux, `--previous-helper-sha256` so Eggup verifies the currently installed generation by exact digest; missing digest proof fails closed for existing files. The command leaves persistent drain set after every outcome, including success, for an explicit later operator `undrain`.

Its JSON output is the operator's only record of what happened, so it reports the release id, the Core artifact disposition, whether manual artifact recovery is required, the lifecycle restoration status, the final observed ownership/state when the adapter can prove them, and the bounded failure detail that explains a rolled-back or retained generation. A `RecoveryRequired` disposition never prints a successful update: it names the manual recovery instead. The post-install check reads the installed daemon's own `version` report and requires an exact match with this release, then, on Linux, runs the installed helper's trust and version check. A rolled-back disposition is therefore a real post-install failure, not a guess.

The Linux sandbox helper trust boundary is defined once, by `eggwork_runner::verify_trusted_helper`, and every caller delegates to it: the node advertises required filesystem isolation exactly when that check accepts the installed helper, and `doctor` reports the same answer the enforcement path would give. A helper that is root- or effective-user-owned, not group/world writable, and inside non-writable ancestor directories is trusted; anything else fails closed, so required isolation stays unsupported rather than degrading.

Drain is stored beside the execution database and is observed by a running node before it accepts each new execution. Local GC previews are read-only; applying GC is bounded and takes an exclusive state lock, so it fails while the node process is running. A live service can use `NodeServer::collect_garbage`, which operates on its existing stores and preserves active references.

## Releases and installation

Releases are produced by [Eggpack](https://github.com/eggstack/eggpack), which owns the release contract, target matrix, builds, qualification, checksum sidecars, `ReleaseManifest`, bootstrap installers, and the generated release workflow. Eggwork owns the node configuration, service lifecycle, drain/update/rollback policy, and the decision about which binaries constitute one installed node.

There are four distinct authorities, and they do not overlap:

```text
Eggpack producer -> ReleaseManifest/assets -> human/release channel -> Eggup/Eggwork local deployment
```

- `release/eggpack/` is the checked-in producer configuration. It is static: no release tag, no source SHA, and no artifact digest is committed.
- `.github/workflows/release.yml` is **generated** from that configuration by `eggpack ci generate`. Never edit it by hand; `eggpack ci check` in CI fails on any drift.
- `install.sh` and `install.ps1` in a release are exact-release **first-install** scripts. They install the binaries, never overwrite, never register a service, never elevate privileges, and never select a release. Service setup afterwards is explicit operator policy through `eggworkd service ...` and the qualified Eggup adapters; updates are Eggup-owned and are not routed through the bootstrap script.
- The staging job prepares a **draft** release only. It creates no tag, moves none, publishes nothing, and refuses to clobber differing assets. Publishing a release remains a maintainer action.

Published targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. Linux releases ship the daemon and its Landlock sandbox helper as one bundle from one source revision, so the pair can never drift. macOS and Windows ship the daemon only.

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

SHA-256 is integrity evidence, not authenticity: it proves the bytes match the staged release, not who published them. Eggup still performs its own ownership and verification checks on the local side.

### Reproducible Windows release binaries

`.cargo/config.toml` scopes one deterministic MSVC link policy to `x86_64-pc-windows-msvc` only:

- `/BREPRO` — `link.exe` derives the PE COFF timestamp deterministically instead of stamping wall-clock time;
- `/DEBUG:NONE` — no CodeView/PDB debug directory is emitted, because a freshly generated PDB contributes a per-build RSDS GUID.

Without both flags a fixed source revision produced two different Windows binaries, and Eggpack correctly refused to replace the differing same-name asset. The deliberate tradeoff is that release Windows binaries carry no PDB; triage of a shipped build uses a local rebuild from the same revision. Linux and macOS targets are untouched, and no workflow may set `RUSTFLAGS` or a per-target equivalent, because Cargo would then use the environment instead of the target-scoped policy.

`.github/workflows/windows-reproducibility.yml` proves the property natively before any release tag exists: it builds the release Windows daemon twice in independent target directories on the pinned toolchain, requires byte-identical SHA-256, and requires no CodeView record in either image (`scripts/verify_windows_reproducibility.py`).

Start here:

- `architecture/distribution.md` — producer/consumer ownership split
- `plans/README.md` — planning system and document hierarchy
- `plans/000-long-term-specification.md` — canonical product/architecture specification
- `plans/002-long-term-roadmap.md` — dependency-ordered long-term roadmap
- `plans/registry.md` — current ready/blocked work and external interface baselines
- `plans/closure/` — implementation and verification evidence

### Qualifying an installation

`scripts/qualify_release.py` is the product-owned operational qualification harness. It retrieves the exact bytes of one staged release through the authenticated GitHub API, verifies every artifact against that release's own `release-manifest.json` and `.sha256` sidecars, then drives the *installed* `eggworkd` through the operator commands a human would use:

```bash
python3 scripts/qualify_release.py install  --release-tag v0.1.1 --installation-root /opt/eggwork
python3 scripts/qualify_release.py installer --release-tag v0.1.1 --installation-root /opt/eggwork
python3 scripts/qualify_release.py service  --installation-root /opt/eggwork
python3 scripts/qualify_release.py update   --release-tag v0.1.1 --release-id v0.1.1 \
  --prior-release-tag v0.1.0 --installation-root /opt/eggwork
```

It never rebuilds a release artifact, never stages or publishes, never mutates a tag, and invokes the product through an argv list with no shell. Installed execution — one authenticated loopback execution through the normal client path, capability admission, and drain admission — is qualified by `crates/eggwork-server/tests/installed_qualification.rs`, an ignored suite that names the installed binary through the environment:

```bash
EGGWORK_QUALIFY_DAEMON=/opt/eggwork/bin/eggworkd \
EGGWORK_QUALIFY_CONFIG=/etc/eggwork/node.json \
EGGWORK_QUALIFY_TLS_DIR=/etc/eggwork \
cargo test -p eggwork-server --features qualification \
  --test installed_qualification -- --ignored --nocapture
```

`.github/workflows/operational-qualification.yml` runs both against a named release on hosted Linux, macOS, and Windows runners and uploads the bounded receipts.

The next implementation handoff is listed in the registry.
