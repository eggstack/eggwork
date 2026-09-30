# Operations and Distribution M003 — Eggpack Producer Packaging Integration

Status: ready for handoff

Repository baseline reviewed before planning:

- Eggwork `a80651e21a6cd4355e7b5e7d98c841fac9f1b173`
- Eggpack current reviewed head `3f95af43f99224755c97161e0c1102a84e713e35`
- Eggpack qualified producer implementation `398cd43bf1597ba49bfc35b5334611aa04b16600` (Build M006 hosted run `36484758546`)
- Eggwork Operations M002 implementation `d5722d9fbbcef0d04c728a83d41322e7d7ca4f73`
- Eggwork Operations M002 closure `plans/closure/operations-distribution/002-resumed-status.md`

Source roadmap:

- `plans/subsystems/operations-distribution-roadmap.md`

Canonical authority:

- `plans/000-long-term-specification.md`
- `plans/001-terminology-and-domain-model.md`
- `plans/002-long-term-roadmap.md`
- `plans/003-planning-process.md`

Primary class: infrastructure/capability

## 1. Objective

Adopt Eggpack as Eggwork's producer-side authority for:

- portable release target/artifact naming;
- exact Cargo package/binary build bindings;
- bounded candidate qualification;
- checksum sidecars;
- ReleaseManifest v1 construction;
- deterministic first-install bootstrap scripts;
- deterministic checked-in release workflow generation and drift checking;
- optional exact-tag GitHub draft staging.

Preserve Eggwork/Eggup ownership of:

- node configuration;
- service registration/lifecycle;
- drain/update/restart policy;
- helper trust/version policy;
- installation-root ownership;
- live update transaction and rollback;
- release selection/acquisition policy, if such policy is added later;
- final human publication.

M003 is producer packaging integration. It is **not** a runtime self-updater and it does not make Eggwork the Eggpack/Eggup interoperability M003 consumer.

## 2. Why this milestone is ready

The producer interfaces originally blocking this milestone are now closed.

Eggpack evidence reviewed:

- ReleaseManifest M001/M001a/M002 closed;
- Build/Qualification M001-M006 closed;
- generated executable release workflow path through CI M003d closed;
- deterministic cross-tool provisioning closed, though this plan does not require it initially;
- bootstrap bundle/archive safety M002/M002a closed;
- Eggup manifest interoperability producer contract closed;
- Build M006 proves build strategy and native qualification are independent and records a green five-target reusable-workflow matrix.

Eggwork evidence reviewed:

- Operations M002 already defines the coherent local deployment unit and Eggup transaction;
- product id is `eggwork`;
- daemon member is `eggworkd`;
- Linux sandbox-helper member is `eggwork-sandbox-helper`;
- daemon/helper replacement, rollback, drain, service ownership, and helper version checks are already qualified on Linux;
- current repository has no competing release workflow: only `.github/workflows/ci.yml` exists.

No Eggpack production-code change is required by the interfaces reviewed for this plan. If implementation discovers such a gap, stop and register the upstream corrective rather than copying producer logic into Eggwork.

## 3. Ownership boundaries

### Eggpack owns

- `DistributionContract` schema and target expansion;
- `PackConfig` / `ReleasePlan`;
- build/qualification bindings;
- bounded Cargo execution;
- final artifact construction;
- SHA-256 sidecars;
- ReleaseManifest v1;
- generated bootstrap installers;
- generated release CI and drift checking;
- draft staging mechanics when enabled.

### Eggwork owns

- which binaries constitute one operational node installation;
- helper-required policy;
- node/service configuration;
- service-manager semantics through Eggup;
- update drain/health/rollback policy;
- platform support claims;
- product-specific candidate semantic checks;
- source/tag/release authorization policy.

### Eggup owns

- local verified installation transactions;
- ownership verification;
- backup/rollback;
- service-manager adapters.

M003 MUST NOT add a second filesystem transaction, service manager, updater, release selector, or deployment receipt implementation.

## 4. Release artifact contract

Add a checked-in Eggpack `DistributionContract` for product id `eggwork`.

Initial canonical targets:

1. `x86_64-unknown-linux-gnu`
2. `aarch64-unknown-linux-gnu`
3. `x86_64-apple-darwin`
4. `aarch64-apple-darwin`
5. `x86_64-pc-windows-msvc`

Aliases should be finite and conventional, for example:

- `linux-x64`
- `linux-arm64`
- `macos-x64`
- `macos-arm64`
- `windows-x64`

### Linux artifact form

Use one Eggpack **bundle** per target containing exactly:

1. daemon:
   - release asset: `eggwork-{version}-{target}`
   - install identity: `eggworkd`
2. sandbox helper:
   - release asset: `eggwork-sandbox-helper-{version}-{target}`
   - install identity: `eggwork-sandbox-helper`

Both entries are executable.

Rationale: Operations M002 already treats the daemon and helper as one coherent generation. A release must not permit independent helper/daemon version selection.

### macOS artifact form

Use one direct daemon artifact:

- `eggwork-{version}-{target}`
- install identity `eggworkd`.

Do not ship the Landlock helper on macOS.

### Windows artifact form

Use one direct daemon artifact:

- `eggwork-{version}-{target}.exe`
- install identity `eggworkd.exe`.

Do not ship the Landlock helper on Windows.

The Eggpack `install` field is release/install identity, not Eggwork service-path authorization. M003 does not change the Operations M002 Linux `CandidateSource` destination policy.

Every release artifact gets an Eggpack-generated SHA-256 sidecar.

## 5. Build policy

Initial M003 uses `BuildStrategy::NativeCargo` for all five targets.

Do not introduce CargoZigbuild or a glibc/macOS compatibility floor merely for consistency with another Eggstack consumer. Eggwork currently has no documented compatibility-floor requirement that would justify that policy.

Toolchain:

- Rust `1.89.0`, matching `rust-toolchain.toml`;
- `--release --locked`;
- exact target triple;
- explicit package and binary binding.

Build bindings:

### Linux bundle

- bundle entry 0 -> package `eggwork-server`, binary `eggworkd`;
- bundle entry 1 -> package `eggwork-sandbox-helper`, binary `eggwork-sandbox-helper`.

### macOS / Windows direct

- direct output -> package `eggwork-server`, binary `eggworkd`.

No workspace-default binary inference.

## 6. Runner and qualification policy

Use `Qualification::Native` for each required target.

Current reviewed runner-class mapping, to be re-verified at implementation time:

| Target | Build/qualification host |
|---|---|
| x86_64-unknown-linux-gnu | Linux x86-64 / `ubuntu-latest` class |
| aarch64-unknown-linux-gnu | Linux AArch64 / `ubuntu-24.04-arm` class |
| x86_64-apple-darwin | Intel macOS / current `macos-15-intel` class |
| aarch64-apple-darwin | Apple Silicon macOS / current `macos-14` class |
| x86_64-pc-windows-msvc | Windows x86-64 / `windows-latest` class |

Provider runner labels are GitHub policy, not portable release identity.

All five targets are initially `SupportTier::Required`. If a required native runner is unavailable at implementation time, stop and revise support/qualification policy explicitly rather than silently downgrading to Structural.

### Core smoke

For every target, core qualification selects the daemon candidate and executes:

`eggworkd version`

with bounded stdout/stderr and timeout.

The command is configuration-free and returns the package version as JSON.

### Linux helper validation

The Linux helper is a second executable in the bundle. Add a bounded Eggwork-owned Python3 consumer validator selecting bundle entry 1 and executing:

`eggwork-sandbox-helper --version`

The validator must:

- receive only the exact candidate path from Eggpack;
- use no network;
- use no shell;
- use no ambient release/version service;
- require exit zero;
- require one bounded non-empty version line;
- compare that line with the workspace package version using bounded source parsing;
- emit no candidate output into Eggpack evidence.

This complements, rather than replaces, core qualification of the daemon.

For macOS/Windows, a consumer validator may additionally parse `eggworkd version` JSON if implementation finds this useful; it is not required if core smoke plus existing CLI tests provide equivalent evidence.

## 7. Checked-in release configuration

Create one product-owned directory, recommended:

`release/eggpack/`

Expected files:

- `distribution.toml`
- `pack.toml`
- `build-bindings.toml`
- `qualification-bindings.toml`
- `consumer-validators.json`
- `bootstrap-install-policy.toml`
- `github-template.json`
- `github-policy.json`
- `workflow-shape.json`

Exact filenames may change if current Eggpack schemas conventionally use another extension, but ownership must remain centralized under one release directory.

Static configuration MUST NOT contain:

- a future release tag;
- a future source SHA;
- a future artifact digest/size;
- credentials;
- a floating Eggpack branch/tag.

The Eggpack tool policy must pin the official repository at one exact reviewed commit.

Initial qualified pin candidate:

- `398cd43bf1597ba49bfc35b5334611aa04b16600`.

The implementation agent must re-check Eggpack current head. A newer pin is acceptable only if the relevant producer contracts and hosted qualification remain intact.

## 8. Generated release workflow

Add one generated checked-in release workflow, recommended:

`.github/workflows/release.yml`

Do not modify ordinary `.github/workflows/ci.yml` into a release workflow.

Use Eggpack reusable workflow-shape rendering so static checked-in bytes contain no future release id/source revision.

Initial trigger policy:

- `workflow_dispatch`;
- one required exact existing `release_tag` input;
- no implicit latest;
- no branch-as-release fallback.

Runtime resolver must:

1. check out the exact requested tag;
2. derive exact HEAD;
3. resolve ReleasePlan/ReleaseCIPlan against the checked-in static config;
4. propagate the same identity documents to every build/qualification/finalization job;
5. fail if any checkout differs from the resolved source revision.

Generated jobs must use:

- immutable action pins;
- least privilege;
- read-only contents permission except the isolated draft-staging job when staging is enabled;
- bounded timeouts and artifact retention;
- exact Eggpack tool pin;
- no arbitrary shell-command extension interface.

Add an ordinary CI drift guard invoking `eggpack ci check` against the checked-in generated workflow.

Do not leave a hand-maintained second release matrix.

## 9. Draft staging and publication boundary

M003 may configure Eggpack GitHub draft staging so the generated workflow is operationally complete, but M003 closure does not require mutating a real public release.

Rules:

- exact existing tag only;
- draft-only;
- no tag create/move/delete;
- no `--clobber`;
- no automatic publication;
- no `make_latest`;
- token only through the provider environment;
- exact asset-set reconciliation.

Final publication remains a maintainer action.

Live draft/rerun/publication evidence belongs to Operations M004.

## 10. Bootstrap installers

Use Eggpack `GeneratedDefault` presentation because Eggwork does not currently own public shell/PowerShell installer wrappers that must be preserved.

Generate deterministic exact-release first-install scripts:

- `install.sh`
- `install.ps1`

Install policy:

- daemon executable mode;
- Linux helper executable mode;
- no data/config member bundled.

Bootstrap is first-install only:

- no overwrite/update;
- no service registration;
- no privilege escalation;
- no release selection/latest;
- no node config generation.

After first install, service setup remains explicit Eggwork operator policy through the existing `eggworkd service ...` surface and qualified Eggup adapters.

Updates remain Operations M002/Eggup-owned and are not redirected through the bootstrap script.

## 11. Release manifest and deployment parity

Add static/product-owned tests that reconcile Eggpack release facts with Eggwork deployment facts without making either library depend on the other at runtime.

Required parity:

- product id == `deployment::PRODUCT_ID`;
- Linux bundle install identities correspond to `MEMBER_DAEMON` and `MEMBER_HELPER`;
- Linux helper is present exactly on Linux release targets;
- daemon present on every release target;
- no config/database/workspace/blob/artifact state appears in release artifacts;
- helper and daemon are always produced from one source revision/release id;
- checksum/manifest identity is producer evidence and does not bypass Eggup ownership verification.

Do not make Eggwork's runtime deployment module parse Eggpack configuration in M003.

## 12. Eggpack/Eggup interoperability distinction

Eggpack's separate Eggup Interoperability M003 is defined as adopting ReleaseManifest evidence in a real runtime consumer that currently duplicates manifest-to-update mapping.

Eggwork does not currently do that:

- it accepts local `CandidateSource` files;
- it has no release discovery/manifest acquisition policy;
- it does not translate a remote ReleaseManifest into an update plan.

Therefore this milestone MUST NOT:

- claim to close Eggpack Eggup Interoperability M003;
- add manifest-driven runtime acquisition merely to satisfy that milestone;
- add an Eggpack runtime dependency to `eggwork-server`;
- move release selection into Eggwork.

Record this cross-repo disposition in Eggpack planning so the two M003 names are not conflated.

## 13. Documentation

Update at minimum:

- root `README.md` release/install section;
- Operations/distribution roadmap;
- registry;
- architecture/deployment ownership documentation as needed;
- contributor/release instructions.

Document four distinct authorities:

`Eggpack producer -> ReleaseManifest/assets -> human/release channel -> Eggup/Eggwork local deployment`

and make clear that bootstrap first install is separate from update/service management.

## 14. Required tests

### Release contract/config

- all five canonical targets resolve;
- Linux resolves to two-member bundle;
- macOS/Windows resolve to direct daemon;
- Windows name has `.exe`;
- checksum sidecars exact;
- case-insensitive name collisions rejected;
- future identity absent from static shape.

### Build bindings

- Linux has exactly daemon + helper selectors;
- macOS/Windows exactly daemon;
- package/bin names exact;
- no hidden workspace/default inference.

### Qualification

- daemon `version` succeeds on each native target;
- Linux helper validator succeeds on exact helper candidate;
- wrong helper output/version fails;
- wrong selector/candidate identity fails;
- missing required target suppresses final release.

### Workflow

- `eggpack ci generate` deterministic;
- `eggpack ci check` zero drift;
- only staging job may request `contents: write`;
- no publication/tag mutation/clobber patterns;
- exact runtime identity flows to every job;
- tool/action revisions immutable.

### Bootstrap/finalization

- Linux bundle first-install projection contains both executables;
- macOS/Windows only daemon;
- generated installers parse on their native shell/PowerShell paths;
- ReleaseManifest round-trips and matches finalized bytes;
- sidecar digests match final bytes.

### Existing Eggwork regression

- Operations M002 deployment tests remain green;
- security M004 regression remains green;
- no direct service-manager invocation;
- execution ownership guard remains green.

## 15. Required verification

Eggwork:

```bash
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
git diff --check
```

Eggpack-driven release configuration:

```bash
eggpack ci generate ... --workflow-shape ...
eggpack ci check ... --workflow-shape ...
```

Use the exact pinned Eggpack revision for both commands.

M003 closure must include hosted build/qualification evidence for the generated five-target workflow or an equivalent non-publishing workflow invocation. It does **not** require a real GitHub draft mutation; that belongs to M004.

## 16. Acceptance criteria

1. Eggpack is the sole checked-in producer authority for the five-target release matrix.
2. Linux releases contain daemon + sandbox helper as one bundle.
3. macOS/Windows releases contain the daemon only.
4. Every required target is natively built and qualified on a matching host.
5. Linux helper exact-candidate validation is gating.
6. ReleaseManifest and checksum sidecars are generated from finalized bytes.
7. Generated exact-release bootstrap scripts install the correct target/member set.
8. Generated release workflow is deterministic and guarded by `eggpack ci check`.
9. No future tag/SHA/digest is checked into static workflow config.
10. No automatic publication or tag mutation exists.
11. Eggwork service/drain/update/rollback policy remains Eggwork/Eggup-owned.
12. No runtime Eggpack dependency is added to Eggwork.
13. Existing Operations M002 and Security M004 tests remain green.
14. Hosted producer build/qualification covers all five required targets.
15. No unresolved high/medium finding remains.

## 17. Stop conditions

Stop and register a corrective/upstream plan if:

- Eggpack cannot represent a Linux daemon/helper bundle plus direct non-Linux daemon targets in one contract;
- generated CI cannot build multiple bundle entries from explicit Cargo bindings;
- native qualification cannot run on a required target host;
- a second arbitrary workflow-command DSL is required;
- product release configuration must embed future release identity;
- bootstrap would need to own service/update semantics;
- implementation requires Eggwork to parse ReleaseManifest at runtime;
- Eggpack producer code must be changed to make the declared configuration valid;
- Windows/macOS packaging would be used to claim service/security qualification without hosted M004 evidence.

## 18. Closure evidence

Create:

- `plans/closure/operations-distribution/003-status.md`

Record:

- exact Eggwork implementation SHA;
- exact Eggpack tool pin and reviewed head;
- checked-in release configuration inventory;
- target/artifact matrix;
- build-binding matrix;
- native runner/qualification matrix;
- Linux helper consumer-validation evidence;
- generated workflow hash/drift result;
- ReleaseManifest + checksum evidence;
- bootstrap projection evidence;
- hosted five-target run identifiers;
- proof no publication/tag mutation occurred;
- Operations M002/Security M004 regression evidence;
- Eggpack Eggup-interoperability non-claim;
- unresolved findings;
- Operations M004 readiness disposition.

## 19. Handoff note

On successful M003 closure, Operations M004 becomes ready. M004 owns live release/draft/rerun evidence, installed first-install smoke, service lifecycle/update/rollback on qualified hosts, and the final macOS/Windows hosted-support disposition.