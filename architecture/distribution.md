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
   policy, helper trust and version policy, the installation root, and any
   release selection or acquisition policy added later.

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

See the [operations and distribution roadmap](../plans/subsystems/operations-distribution-roadmap.md)
for milestone status and the [closure records](../plans/closure/operations-distribution/)
for evidence.
