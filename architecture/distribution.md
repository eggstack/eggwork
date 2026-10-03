# Release and distribution ownership

Producing a release and consuming one are separate authorities. Eggwork sits on
the consuming side of a boundary it does not own.

## Four authorities

```text
Eggpack producer -> ReleaseManifest/assets -> human/release channel -> Eggup/Eggwork local deployment
```

1. **Eggpack** builds the release: target and artifact naming, exact Cargo
   bindings, bounded candidate qualification, SHA-256 sidecars, the
   `ReleaseManifest`, deterministic first-install scripts, and the generated
   release workflow.
2. **The release channel** is a published GitHub draft turned public by a human.
   Eggwork never publishes, never creates or moves a tag, and never selects
   "latest".
3. **Eggup** performs the local verified installation transaction, ownership
   verification, backup/rollback, and service-manager adapters.
4. **Eggwork** owns node configuration, service lifecycle, drain/update/restart
   policy, helper trust and version policy, the installation root, its own
   release *build* determinism policy, and any release selection or acquisition
   policy added later. Update orchestration uses Eggup's lifecycle transaction;
   Eggwork does not manually restart a service after a Core transaction receipt.

A fifth, smaller authority is worth naming because it is easy to misplace:
`.cargo/config.toml` is Eggwork product build policy. Eggpack cannot remove
nondeterminism that the product's own link flags introduce, and it is right to
fail closed when two builds of one source revision disagree.

## What Eggpack owns in this repository

`release/eggpack/` is the checked-in producer configuration:

| File | Authority |
|---|---|
| `distribution.toml` | the release contract: product id, published targets, aliases, asset names, install identities, checksum sidecars |
| `pack.toml` | per-target build strategy, build/qualification host, toolchain, compatibility floor, qualification intent, support tier |
| `build-bindings.toml` | explicit Cargo package and binary per logical contract slot |
| `qualification-bindings.toml` | the fixed-argument core candidate smoke per target |
| `consumer-validators.json` | Eggwork-owned bounded validation of the exact candidate after core qualification |
| `install-policy.toml` | first-install mode per bundle member |
| `github-template.json` | the static draft template resolved at run time |
| `github-policy.json` | runner labels, immutable action pins, the pinned Eggpack tool, explicit input paths, staging policy |
| `workflow-shape.json` | the static shape the checked-in release workflow is rendered from |

`.github/workflows/release.yml` is **generated** from that configuration. It is
never hand-edited; `eggpack ci check` in the ordinary CI workflow fails on any
drift. There is no second hand-maintained release matrix.

## Invariants

- The static configuration contains no release tag, no source SHA, and no
  artifact digest or size. Release identity is resolved at run time from the
  exact dispatched tag and the checked-out `HEAD`, then propagated to every job,
  which verifies it before doing release work.
- Linux releases publish the daemon and its Landlock sandbox helper as one
  Eggpack bundle built from one source revision, matching the coherent
  deployment unit in `deployment::install_unit_matrix`. A release cannot permit
  independent helper and daemon version selection.
- macOS and Windows publish the daemon alone. The Landlock helper is not shipped
  there, and `install_unit_matrix(false)` marks it not required.
- No config, database, workspace, blob, or artifact state is ever published.
  Those belong to the installation root, not to a release artifact.
- The contract's `install` field is release/install identity. It is not
  service-path authorization; the Operations M002 `CandidateSource` destination
  policy is unchanged.
- The consumer validator is bounded, offline, shell-free, and reads only the
  exact candidate path Eggpack hands it plus the checked-in workspace version.
  It emits no candidate output into release evidence.
- The workspace version, the daemon/helper version output, the release id, the
  asset names, and the `ReleaseManifest` must all agree. A partial bump would
  make a tag, its assets, and its binaries disagree, which the producer validator
  and a static parity test both reject.
- The `x86_64-pc-windows-msvc` link policy is deterministic
  (`/BREPRO` + `/DEBUG:NONE`), is scoped to that target alone, and cannot be
  replaced by an environment rustflags setting. A native two-build
  byte-identity proof runs before a release tag is created.

## What this milestone deliberately does not do

- `eggwork-server` has no runtime Eggpack dependency and never parses
  `release/eggpack/`, a `ReleaseManifest`, or a distribution contract. Static
  parity between the two is asserted by tests in
  `crates/eggwork-server/tests/release_contract.rs`.
- Eggwork does not do manifest-driven release discovery or acquisition. It
  accepts local candidate files. That is Eggpack's separate runtime-consumer
  interoperability milestone, not this one.
- Bootstrap installers are first-install only. They do not update, register a
  service, elevate privileges, select a release, or generate node config.
  Updates remain Eggup-owned.
- A staged draft is not a public bootstrap source. Public first-install
  qualification requires an explicit maintainer publication action; Eggwork's
  release workflow never publishes automatically.
- Platform service policy is selected explicitly: systemd scope and unit path,
  launchd domain/target and plist path, or the finite Windows SCM start type.
  Native service support is claimed only for backends with live hosted
  lifecycle evidence recorded in the Operations closure record.
- A service backend with no live evidence fails closed rather than staying
  enabled. Windows service management is the current case: the daemon has no
  service-control dispatcher, so an SCM registration can never reach Running and
  every mutating verb is refused before any mutation. The adapter and the
  recorded policy are retained; only the operator surface is closed.
- First install and update are distinct operations. First-install installers
  refuse existing destinations; updates use Eggup's verified local transaction
  and preserve drain until an operator explicitly clears it. The deployment
  library accepts local candidates and does not discover releases; M004's
  command-line `deployment apply` surface accepts only local daemon/helper
  paths. Replacements require exact previous-generation digest proof for every
  member; Linux release updates always include both daemon and helper.

## Qualifying an installation

`scripts/qualify_release.py` and `crates/eggwork-server/tests/installed_qualification.rs`
are the product-owned qualification harness. Both consume an already-staged
release: they verify bytes against that release's own manifest and sidecars, and
they exercise the installed binary through the ordinary operator surface. Neither
rebuilds a release artifact, stages or publishes anything, mutates a tag, or adds
a second producer release matrix. `.github/workflows/operational-qualification.yml`
runs them on hosted runners and uploads bounded receipts.

See the [operations and distribution roadmap](../plans/subsystems/operations-distribution-roadmap.md)
for milestone status and the [closure records](../plans/closure/operations-distribution/)
for evidence.
