# release and qualification

A release of Eggwork is produced by Eggpack, a separate authority, and is consumed
by Eggup/Eggwork as a local installation. This document covers the two halves of
that boundary that Eggwork actually owns: the static producer configuration
checked in under `release/eggpack/`, and the product-owned qualification surface —
the operational harness (`scripts/qualify_release.py`), the consumer validators
that gate release evidence, the opt-in installed-execution suite, and the hosted
workflow that runs them. It also records the release-lineage failures that shaped
those surfaces, because most of the guards here exist because something once
shipped, failed, or lied.

## The four authorities

```text
Eggpack producer -> ReleaseManifest/assets -> human/release channel -> Eggup/Eggwork local deployment
```

1. **Eggpack** builds the release: target set, artifact naming, exact Cargo
   bindings, bounded candidate qualification, SHA-256 sidecars, the
   `ReleaseManifest`, the first-install scripts, and the generated workflow.
2. **The release channel** is a GitHub draft that a human turns public. Eggwork
   creates no tag, moves none, publishes nothing, and never selects "latest".
3. **Eggup** performs the local verified installation transaction, ownership
   verification, backup/rollback, and service-manager adapters.
4. **Eggwork** owns node configuration, service lifecycle, drain/update/restart
   policy, helper trust and version policy, its own installation root, its own
   release *build* determinism policy, and any release selection or acquisition
   policy added later.

Eggwork sits on the **consuming** side of a boundary it does not own: it never
parses a distribution contract, never reads a `ReleaseManifest`, and has no
runtime dependency on Eggpack. That is not a style preference — it is asserted
(§ [Static parity test](#static-parity-test)).

A fifth, smaller authority is easy to misplace: `.cargo/config.toml` is **Eggwork
product build policy**. Eggpack owns the artifacts but cannot remove
nondeterminism the product's own link flags introduce, and it is right to fail
closed when two builds of one source revision disagree.

The full split — including what Eggwork may still add later — is in
[distribution.md](distribution.md); it is not restated here.

## Producer configuration

`release/eggpack/` contains nine files, 593 lines total. Every one is static.

| File | Authority |
|---|---|
| `distribution.toml` | the release contract: product id, published targets, aliases, asset names, install identities, checksum sidecars |
| `pack.toml` | per-target build strategy, build/qualification host, toolchain, compatibility floor, qualification intent, support tier |
| `build-bindings.toml` | explicit Cargo package and binary per logical contract slot |
| `qualification-bindings.toml` | one fixed-argument core candidate smoke per target |
| `consumer-validators.json` | Eggwork-owned bounded validation of the exact candidate |
| `install-policy.toml` | first-install mode per bundle member |
| `github-template.json` | static draft release template |
| `github-policy.json` | runner labels, immutable action pins, pinned Eggpack tool revision, explicit input paths, staging policy |
| `workflow-shape.json` | the shape `.github/workflows/release.yml` is rendered from |

### Published target matrix

`distribution.toml` declares exactly five targets, all `support = "required"` in
`pack.toml`, each built with `strategy = "native_cargo"` on a host whose OS and
architecture match the target and qualified with `qualification = "native"` on
that same host. Toolchain is pinned to `rust = "1.89.0"` for all five, matching
`rust-toolchain.toml`'s `channel = "1.89.0"`. No compatibility floor is declared
(`floor = { kind = "none" }`) for any target — Eggwork documents no glibc or macOS
deployment floor, so introducing one (or CargoZigbuild) for cross-repo consistency
would be an unsupported policy change.

| Triple | Alias | Asset form | Installed as | Pairs |
| --- | --- | --- | --- | ---: |
| `aarch64-apple-darwin` | `macos-arm64` | direct | `eggworkd` | 1 |
| `aarch64-unknown-linux-gnu` | `linux-arm64` | bundle | `eggworkd`, `eggwork-sandbox-helper` | 2 |
| `x86_64-apple-darwin` | `macos-x64` | direct | `eggworkd` | 1 |
| `x86_64-pc-windows-msvc` | `windows-x64` | direct | `eggworkd.exe` | 1 |
| `x86_64-unknown-linux-gnu` | `linux-x64` | bundle | `eggworkd`, `eggwork-sandbox-helper` | 2 |

Asset names are templated, not literal:
`{product}-{version}-{target}` for Unix, `{product}-{version}-{target}.exe` for
Windows. Checksum sidecars are `{asset}.sha256` for every target. Seven binaries
plus seven sidecars plus `release-manifest.json` plus `install.sh` and `install.ps1`
is the 17 assets a full release publishes — the count recorded for the `v0.1.4`
staged draft.

### Per-platform bundle composition

The two Linux targets ship an Eggpack **bundle** of exactly two executables from
one source revision, so the daemon and its Landlock sandbox helper can never be
selected from different release ids. macOS and Windows ship the daemon alone
because the Landlock helper is not shipped there. `install` is release/install
identity, not service-path authorization; the M002 Linux `CandidateSource`
destination policy is unaffected.

`install-policy.toml` therefore has entries only for the two Linux targets, and
marks every bundle member `"executable"`. No data, config, database, workspace,
blob, or artifact member is bundled, so nothing is declared `Data`. macOS and
Windows are single direct daemon artifacts and take Eggpack's direct-release
bootstrap behaviour.

`build-bindings.toml` names the Cargo package and binary for every slot. There is
no workspace-default binary inference anywhere:

```toml
"x86_64-unknown-linux-gnu" = [
  { selector = { kind = "bundle_entry", index = 0 }, package = "eggwork-server", binary = "eggworkd" },
  { selector = { kind = "bundle_entry", index = 1 }, package = "eggwork-sandbox-helper", binary = "eggwork-sandbox-helper" },
]
```

Bundle entry order is the contract order in `distribution.toml`: entry 0 is the
daemon, entry 1 is the helper.

`qualification-bindings.toml` gives every target one smoke that always selects the
*daemon* and runs a fixed, configuration-free argv:

```toml
selector = { kind = "direct" }          # or bundle_entry index 0 on Linux
argv = ["version"]
timeout_ms = 10000
stdout_limit = 8192
stderr_limit = 8192
```

No node config, no helper, no network. The helper is a second executable in the
bundle and is proven by the bounded consumer validator instead, not by a second
qualification implementation.

`github-policy.json` fixes the runner labels (`ubuntu-latest`,
`ubuntu-24.04-arm`, `macos-15-intel`, `macos-14`, `windows-latest`, all with
`cargo_zigbuild: false`), pins every action to an immutable commit
(`actions/checkout@11d5960a…`, `dtolnay/rust-toolchain@2c7215f1…`,
`actions/upload-artifact@ea165f8d…`, `actions/download-artifact@d3f86a10…`), pins
the Eggpack tool to `revision = "32a0903936fcc283863e0bfb86151b13b4d75ce9"`
(`package = "eggpack-cli"`), triggers on `workflow_dispatch` only, and names all
seven release inputs explicitly. The staging block sets `tag_source =
"dispatch_input"` and three inputs (contract, install policy, draft template) with
`receipt_retention_days: 7`.

`github-template.json` is a static draft template: `owner = "eggstack"`,
`repository = "eggwork"`, `title_prefix = "eggwork "`, `prerelease = false`,
`token_env = "GITHUB_TOKEN"`, and a body that states plainly that maintainer
review and publication are required before the release becomes public. It carries
no `tag` and no `release_id` — asserted by a test.

## Identity-free by construction

No release tag, source SHA, artifact digest, or artifact size is committed under
`release/eggpack/`. Release identity is expressed only through `{product}`,
`{version}`, and `{target}` templates.

The reason is simple: **a committed identity is a lie the moment the branch
moves.** A checked-in `release_id` or `sha256` describes one instant that
nothing in the build re-checks; the first commit after it lands makes it
descriptive of a revision nobody will build. Making identity a *derived* value
means it is either correct or the run fails.

Identity is resolved at run time, once per run, by a preflight `_resolve-release`
invocation that takes the dispatched tag and the checked-out `HEAD`:

```yaml
'eggpack' 'ci' '_resolve-release' '--contract' 'release/eggpack/distribution.toml'
  '--pack-config' 'release/eggpack/pack.toml' '--build-bindings' 'release/eggpack/build-bindings.toml'
  '--qualification-bindings' 'release/eggpack/qualification-bindings.toml'
  '--consumer-validators' 'release/eggpack/consumer-validators.json'
  '--selected' 'linux-x64,linux-arm64,macos-x64,macos-arm64,windows-x64'
  '--tag' '${{ inputs.release_tag }}' '--source-revision' "$head_sha"
  '--template' 'release/eggpack/github-template.json'
  '--output-plan' './eggpack-runtime/release-plan.json' ...
```

The preflight writes its result to workflow-private storage
(`eggpack-runtime/release-plan.json`, `release-ci-plan.json`, `github-draft.json`),
uploads it once as the artifact named `eggpack-runtime-identity`, and **18 jobs**
download it and re-verify their checked-out source against it via `'_verify-source'`
with `--release-plan 'eggpack-runtime/release-plan.json'`. The count is asserted
exactly, so a new job cannot skip verification and a removed one cannot go
unnoticed. `git rev-parse --verify HEAD^{commit}` is in the workflow, no
all-zeros source revision may be rendered, and the static path
`release/eggpack/runtime/release-plan.json` must not appear in the generated
workflow.

The static-parity test enforces the inverse: no asset or install template may
contain the current version string, any template containing `{` must contain both
`{version}` and `{target}`, `workflow-shape.json` must not contain `release_id`,
`source_revision`, `release_tag`, `sha256":`, or `digest`, and the draft template
must have no `tag` or `release_id` key.

## The generated workflow

`.github/workflows/release.yml` is **generated** by `eggpack ci generate` from
`release/eggpack/workflow-shape.json`. Never edit it by hand; `eggpack ci check`
in ordinary CI fails on drift. `workflow-shape.json` embeds the target list, the
`selected_aliases`, the build bindings, the qualification bindings, the consumer
validators, and the staging block (`provider: git_hub_draft`, `tag_source:
dispatch_input`, `required: true`) — everything needed to render the workflow
without consulting the network.

There is **no second hand-maintained release matrix**. A test walks
`.github/workflows/`, requires `release.yml` to be the only file with that name,
and asserts that no *other* workflow contains `--target '<triple>'` for any
required triple. A candidate cannot smuggle a second release pipeline in through a
new workflow file.

Two other structural guards on the rendered artifact:

- **Permissions.** `contents: write` appears exactly once and only after the
  `\n  stage:\n` marker — exactly one job may write repository contents.
  `contents: write-all`, `packages: write`, `deployments: write`,
  `pull-requests: write`, and `id-token: write` are all absent.
- **No tag or publication capability.** The workflow must not contain
  `--clobber`, `gh release`, `make_latest`, `git tag`, `git push origin`,
  `actions/github-script`, or `curl`. It must contain `workflow_dispatch:` and
  `release_tag:` and must *not* contain a `push:` trigger: staging is manual
  dispatch only, against an exact existing tag. `GITHUB_TOKEN` appears exactly
  twice, both inside the staging job, and reaches the tool only as
  `GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}` — never in argv.

Every job also names `--contract 'release/eggpack/distribution.toml'` explicitly
and every required triple has a build, qualification, and validation path.

`.cargo/config.toml` is the fifth, smaller authority: Eggwork product build
policy, which Eggpack cannot override. See
[Windows determinism](#windows-determinism-and-the-pdb-tradeoff).

## Draft-only staging

The staging job prepares a **draft** release and nothing else:

- it creates no tag and moves none — the tag already exists and is supplied as a
  dispatch input (`tag_source = "dispatch_input"`);
- it publishes nothing — `workflow-shape.json` fixes
  `provider = "git_hub_draft"` and `github-template.json` sets
  `prerelease = false` on a draft whose body states that maintainer review and
  publication are required;
- it cannot clobber: `--clobber` is forbidden in the rendered workflow, and
  Eggpack's same-name digest rule means a differing asset is a failure, not an
  overwrite (see [Real failure modes](#real-failure-modes-on-record));
- it is the only job with `contents: write` and the only place a token is bound.

Publication is a maintainer action outside this repository's automation.

The boundary exists because the failure it prevents is not recoverable by a
retry. A tool with write scope and a release name is one `make_latest` away from
making a candidate public and permanent, and no downstream check can un-publish
bytes a user has already downloaded. A draft is cheap to inspect, cheap to
discard, and forces a human to look at the manifest before the bytes mean
anything.

The same reasoning is why the harness reads a draft rather than the public
release: the pre-publication candidate is the thing worth qualifying, and reading
it must not require publishing it first.

## First-install vs update

`install.sh` and `install.ps1` in a release are exact-release **first-install**
scripts. They install the binaries and nothing more:

- they never select a release — the release is an input, not a choice;
- they never overwrite an existing file;
- they never register a service;
- they never elevate privileges;
- they never generate node configuration.

Service setup afterwards is explicit operator policy through `eggworkd service ...`
and the qualified Eggup adapters. Updates are Eggup-owned and are never routed
through the bootstrap script. `install-policy.toml` says this in its own header:
it "describes first install only … never selects a release, never overwrites an
existing file, never registers a service, never elevates privileges, and never
generates node configuration."

Why keep bootstrap this narrow: a script that installs, registers, and upgrades in
one pass has no defined answer for "the user already has a node". Either it
silently replaces a running installation — destroying the operator's explicit
state and any local configuration the product never owned — or it refuses and
fails at the worst moment. Splitting first install from update means the first is
idempotent-and-refusing and the second is a transaction with receipts, backup, and
rollback. See [deployment-lifecycle.md](deployment-lifecycle.md) for the consumer
transaction, ownership verification, and drain/update/rollback policy, and
[operations-cli.md](operations-cli.md) for the operator verbs.

The Linux bundle is installed with POSIX executable mode for every member,
because HTTPS carries no mode bits and the install identities are executables by
contract.

## Static parity test

`crates/eggwork-server/tests/release_contract.rs` (1,390 lines) reconciles the
producer configuration against Eggwork's own `deployment::install_unit_matrix`
and the workspace version. It reads the checked-in files as text — no network, no
Eggpack, no build — and runs in ordinary `cargo test`.

What it asserts, by group:

**The contract.** Exactly the five canonical targets are published. Every
contract alias resolves to its own triple. The Linux targets publish one
two-member bundle; non-Linux targets publish the daemon directly; Windows
publishes an `.exe`-suffixed executable. Every release file has an exact
`.sha256` sidecar, and expanded release names are unique under case-insensitive
comparison.

**Parity with the consumer.** Install identities match the deployment install
unit. The helper ships exactly on the Linux release targets and the daemon ships
on every one. The Linux helper and daemon share one release identity. No release
asset carries node state. The Linux helper validator agrees with
`deployment`'s version policy — the same identity
`deployment::check_helper_compatibility` requires at update time.

**Policy.** Every target is built and qualified natively; the toolchain policy
matches the checked-in `rust-toolchain.toml`. Build bindings name exact packages
and binaries, and those packages and binaries exist in this workspace. Core
qualification smoke always selects the daemon with a fixed argv, and the
qualification selector always names a bound build output. Consumer validation is
gating and selects the Linux helper. Install policy marks every bundle member
executable and nothing else.

**Identity-free.** `static_release_configuration_embeds_no_future_release_identity`.

**The rendered artifact.** The embedded workflow shape matches the checked-in
configuration. The GitHub policy pins every runner, action, and the tool
immutably, and the draft policy is draft-only and needs no wrapper. The generated
workflow is derived only from static configuration, grants `contents: write` to
only the staging job, never publishes or mutates tags, pins actions and installs
the pinned tool, and propagates one runtime identity to every job. No second
hand-maintained release matrix exists.

**Build determinism.** The MSVC build policy is deterministic and target-scoped,
and no workflow or environment can replace the target-scoped rustflags.

**Sealed boundary.** `the_deployment_module_never_parses_eggpack_configuration`
reads `src/deployment.rs`, `src/operations.rs`, and `src/bin/eggworkd.rs` and
requires none of them to mention `release/eggpack`, `ReleaseManifest`,
`release-manifest`, `DistributionContract`, or `eggpack::`. **`eggwork-server`
has no runtime Eggpack dependency and never parses a producer contract, a
`ReleaseManifest`, or a distribution contract.** Release selection and
acquisition stay outside this milestone.

**One coherent version.** `the_release_version_is_one_coherent_workspace_identity`
requires `Cargo.toml`'s `[workspace.package].version` to equal the locked version
of all five workspace members, requires every member manifest to *inherit* rather
than restate the version, and requires the version to carry no pre-release
suffix. The comment states the reason: the daemon and helper both compile
`CARGO_PKG_VERSION`, the producer validator compares `eggworkd version` against
`[workspace.package].version`, and the release asset names carry the same string.
**A partial version bump would make a tag, its assets, and its binaries
disagree** — which both the producer consumer-validator and this test reject.
That is exactly the `v0.1.4` failure (§ below).

One more test earns its place: `linux_only_dependencies_never_enter_the_non_linux_build_graph`.
Run `36784830178` proved it the hard way — `landlock` is Linux-only and its
unconditional declaration broke the macOS and Windows daemon builds. The test
requires `landlock` and `nix` to be gated behind exactly one platform table each.

## The qualification harness

`scripts/qualify_release.py` (1,638 lines) is the product-owned operational
qualification harness. It is *not* a producer tool: Eggpack owns release
construction, assets, sidecars, and publication.

### Stages

| Stage | What it does |
|---|---|
| `inventory` | the only consumer-shaped read; enumerates what one exact release declares, and is the only stage that may run `--public` |
| `install` | downloads and verifies the exact release bytes and installs them into `--installation-root` |
| `installer` | drives the generated exact-release `install.sh` / `install.ps1` |
| `service` | the native service-lifecycle matrix on the host platform (Linux systemd, macOS launchd, Windows disposition) |
| `update` | drain, update, and rollback against real generations; requires `--release-id` |

Every stage except `inventory` requires `--installation-root`. `update`
additionally requires `--release-id`. `--target` defaults to the host triple.
`--release-tag` defaults to `EGGWORK_QUALIFY_TAG` or `v<workspace version>` — read
from the checked-in manifest rather than hard-coded, because a stale default would
silently qualify the *previous* release instead of failing. `--scope` selects
`user` or `system`. Update-only inputs include `--candidate-daemon` /
`--helper-candidate` (rehearsal only) and `--prior-candidate-daemon` /
`--prior-release-tag` (a local prior-generation daemon whose post-install check
must fail, exercising the real rollback path).

### Exact retrieval

Retrieval goes through the authenticated GitHub API, never a public URL:

- `_resolve_release` lists `repos/<repo>/releases` page by page and matches
  `tag_name` exactly, rather than using the by-tag endpoint, because that
  endpoint resolves only *published* releases and the candidate is a draft. The
  same code path therefore works before and after publication, which is what
  makes the pre- and post-publication matrices comparable.
- `fetch_release` re-checks that the resolved release's `tag_name` is exactly the
  requested tag, so a moved tag cannot quietly qualify different source.
- `download_asset` writes to a `.partial` file and only then `replace()`s the
  destination, enforces `MAX_ASSET_BYTES` while streaming, and retries **only**
  5xx and 429. A 4xx is a bug in the request, and retrying it would hide that
  bug behind a delay. Transport errors, timeouts, and 5xx *are* retried: the
  release API is a shared dependency that occasionally answers a well-formed
  request with a 5xx, which is indistinguishable from a real product failure.

### Double verification against the release's own manifest

Every binary artifact is verified **twice**, both times against data belonging to
that release:

1. against the `.sha256` sidecar shipped beside it — the sidecar must be a
   regular file of at most 4 KiB, contain exactly two whitespace-separated
   fields, name the artifact, and the streamed SHA-256 must match;
2. against `release-manifest.json` — the manifest must declare
  `release_id == tag` and `product_id == "eggwork"`; each asset name must carry
   its target suffix (with the Windows `.exe` handled explicitly), and the local
   size and SHA-256 must both equal the manifest's recorded values.

Asset names are checked to carry their target, so a manifest that names an
artifact for the wrong platform fails rather than installing. The manifest's
`source_revision` is carried into the `Release` record. Generated installers are
size-bounded so a pathologically large `install.sh` is rejected.

### Invocation, and what it never does

Every child process is invoked from an argv list with `shell=False`. The single
documented exception is the generated release installer itself, which *is* a
shell script. Child output is bounded on both streams, timeouts are explicit, and
`ProcessResult.json()` parses a single JSON object or fails closed.

The harness never:

- **rebuilds a release artifact** — it downloads and verifies, never compiles;
- **stages or publishes a release** — it makes only GET requests; the only two
  `urllib` request sites in the file are the release listing and the asset
  download, both read-only;
- **mutates a tag** — it never creates, moves, or references `latest`;
- **adds a second producer release matrix** — the five-target build,
  qualification, validation, gate, aggregate, and staging matrix stays exclusively
  in the Eggpack-generated `release.yml`;
- **leaves a mutated native service behind** — service stages clean up on
  failure;
- **escalates privileges** — service definitions are written to user-scoped
  locations (systemd user unit, launchd `LaunchAgents` plist).

### Token scoping

A staged candidate is a **draft**, and GitHub's REST API does not expose drafts
to the Actions `GITHUB_TOKEN` — the list endpoint returns them only to a token
with repository scope. The harness therefore reads a scoped secret from
`RELEASE_QUALIFICATION_TOKEN`. The value never appears in argv, stdout, or a
receipt, and the workflow's own `permissions` stay at `contents: read`, so the
workflow cannot create, move, or publish anything even with a broader token
available.

`--public` sets `PUBLIC_MODE`, and it *fails closed* on a set token: a tokened
fetch must never masquerade as public bootstrap evidence. `--public` is also
rejected for every stage except `inventory`, because letting it qualify an
install would quietly downgrade a privileged pre-publication check to an
anonymous one.

### Receipts

A `Receipt` is `{"schema_version", "stage", "facts", "steps"}` with sorted keys.
`record()` appends a named step and folds its facts; `expect()` records an
outcome and raises `QualificationFailure` when the condition is false, so a stage
cannot report a fact it did not verify. On failure `main()` prints
`QUALIFICATION FAILED (<stage>): …` and exits 2.

## Installed-execution qualification

`crates/eggwork-server/tests/installed_qualification.rs` (809 lines) drives the
*installed* `eggworkd` through the production client path. Everything else in the
M004 surface exercises the product's own operator commands; this suite exercises
the one property those commands cannot reach — that the installed release binary,
exactly as staged and digest-verified, accepts a real authenticated loopback
execution and admits or refuses capabilities truthfully.

The whole file is behind `#![cfg(feature = "qualification")]`, an opt-in feature
declared in `crates/eggwork-server/Cargo.toml` (it is empty — the suite is
entirely opt-in, not additive). The `qualification` feature is not in the
default build of the shipped daemon: the suite is a test-only subject, and
shipping test scaffolding and a fixture-materialising test in the binary an
operator installs would be a distribution liability for zero product value. An
ordinary `cargo test --workspace` has no installed release to drive, and a test
that silently passes because its subject is missing is worse than no test.

### The `EGGWORK_QUALIFY_*` contract

```text
EGGWORK_QUALIFY_DAEMON   absolute path to the installed eggworkd
EGGWORK_QUALIFY_CONFIG   absolute path to the qualification node config
EGGWORK_QUALIFY_TLS_DIR  directory holding ca.pem / client.pem / client-key.pem
EGGWORK_QUALIFY_RECEIPT  optional path to write the JSON receipt
EGGWORK_QUALIFY_PLATFORM `linux` | `macos` | `windows`
```

The fixture test additionally reads `EGGWORK_QUALIFY_ROOT` (required, must be an
absolute path), `EGGWORK_QUALIFY_BIND` (default `127.0.0.1:0`), and
`EGGWORK_QUALIFY_HELPER` (default `<root>/bin/eggwork-sandbox-helper`). A missing
subject is an **error, not a skip**.

### The TLS fixture

`qualification_fixture` materialises the loopback fixture with `rcgen`, the
repository's existing mTLS fixture library: a self-signed CA
(`CN = eggwork-qualification-ca`, unconstrained, `KeyCertSign` + `CrlSign`), a
node identity for `localhost` / `127.0.0.1`, and a client identity
`eggwork-controller`. The client certificate's own SHA-256 DER digest becomes the
grant, because the node's client-CA verifier hashes the verified leaf.

The reason for `rcgen` is stated in the file: the harness must not depend on
whichever OpenSSL or LibreSSL a runner happens to ship. Those disagree about
certificate version, extension flags, and key encodings, and a fixture the
installed binary rejected would look like a product defect. `rcgen` produces
identical identities on every host, so the comparison is between hosts and not
between TLS libraries.

The node refuses a config whose TLS material is group/world accessible or whose
state directories are not owner-only, so the fixture is written with exactly the
permissions a real installation uses: `ca.pem` / `node-chain.pem` / `client.pem`
at `0o644`, `node-key.pem` / `client-key.pem` at `0o600`. On non-Linux hosts the
`sandbox_helper` key is omitted entirely, because declaring it would advertise a
required capability that does not exist.

### Invocation and what it proves

```bash
cargo test --features qualification -p eggwork-server \
  --test installed_qualification -- --ignored --exact qualification_fixture
cargo test --features qualification -p eggwork-server \
  --test installed_qualification -- --ignored --exact \
  installed_release_admits_execution_and_refuses_unsupported_isolation
```

Both are `#[ignore]`d, the first with the reason `materialises the qualification
fixture; run with --ignored`.

The second test:

1. Runs the installed binary's own `config validate` — the operator-facing
   configuration must be accepted by the installed binary's **loader**, not merely
   parse as JSON — and `doctor`, requiring `ready == true`.
2. Starts the installed node as a bounded child, reserves a loopback port, and
   builds an `eggwork_client::NodeClient` over mTLS. The code path under test is
   the production client path, not a bespoke probe.
3. **Proves one authenticated loopback execution**: state `Succeeded`, stdout
   exactly `eggwork-qualification` (CRLF on Windows, LF elsewhere), and
   `cleanup_warning` null.
4. **Proves capability admission** in whichever direction applies. If the node
   advertises `isolation.landlock.workspace-rw.v1`, a required-isolation execution
   must succeed with `sandbox.status == "applied"`, *and* a second execution whose
   target writes outside the workspace must not succeed and must leave the
   side-effect marker absent — the sandbox is real, not merely reported. If the
   node does not advertise it, the request must be refused with a
   `capability_mismatch` / 409 and the marker must not exist, proving refusal
   happened *before* the target ran.
5. **Proves drain admission**: `drain` succeeds, a new execution is refused, the
   refusal is not a transient — `undrain` is an explicit operator act — and the
   same node admits work again afterwards.

Bounds are tight: 60 s to report ready, 60 s per execution, 8 KiB of stdout per
execution, 64 KiB of ready output. The result is a receipt
(`{"schema_version", "stage": "execution:<platform>", "installed_daemon", "facts"}`)
written to `EGGWORK_QUALIFY_RECEIPT` when set.

## Consumer validators

`consumer-validators.json` binds one bounded validator per target:

| Target | Selector | Script |
| --- | --- | --- |
| `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc` | `direct` | `scripts/validate-daemon-version.py` |
| `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu` | `bundle_entry` index 1 | `scripts/validate-sandbox-helper.py` |

All five run under `python3` with `timeout_ms: 60000` and `stdout_limit` /
`stderr_limit` of 65536.

`scripts/release_candidate_probe.py` is the shared library. Its contract is
narrow and stated in its own docstring: Eggpack invokes each validator as
`python3 <script> <candidate>` from the checked-out repository root, and the
single argument is the exact candidate path Eggpack selected from the transferred
build handoff for that target. Nothing else is accepted.

- **Bounded.** `MAX_SOURCE_BYTES` 1 MiB for the manifest, `MAX_VERSION_BYTES` 64
  for the version string, 64 KiB each for candidate stdout and stderr, and a
  30 s candidate timeout. `decode_bounded` rejects non-UTF-8 output and any
  control byte other than `\r\n\t`.
- **Offline.** No network, no shell, no ambient Eggwork, Eggup, or
  service-manager consultation. The expected version is read from the checked-in
  `Cargo.toml`'s `[workspace.package].version` via `tomllib`, and the manifest
  must be valid UTF-8 TOML with an in-bounds package version.
- **Shell-free.** `subprocess.run([str(candidate), *argv], shell=False,
  stdin=DEVNULL)`, with a minimal allowlist environment: on POSIX
  `PATH, HOME, TMPDIR, LANG, LC_ALL`; on Windows `SYSTEMROOT, SystemRoot, COMSPEC,
  PATHEXT, WINDIR`.
- **No candidate output in release evidence.** Candidate output is deliberately
  never returned. Eggpack records only the validator's exit status; the helpers
  print fixed reason phrases (`validation failed: …` / `validation passed`) so a
  failing run cannot leak candidate bytes into workflow logs or release evidence.

**`validate-daemon-version.py`** runs `eggworkd version` on the candidate and
requires stdout to parse as a JSON *object* with a non-empty string `version` field
equal to the workspace version. This turns "the candidate exits zero" into a
version-coherence claim on **every** required target, not only Linux.

**`validate-sandbox-helper.py`** runs `eggwork-sandbox-helper --version` and
requires exactly one bounded non-empty version line equal to the workspace
version — the same identity `deployment::check_helper_compatibility` requires at
update time, so a helper whose version drifts from the daemon is refused before
the release can ship.

The distinction matters: **Eggpack core qualification remains the authority for
the daemon.** The consumer validator is gating release evidence, not a second
qualification implementation. It exists because a validator is the only place
that can compare a candidate's *reported* version against the checked-in
workspace version on all five targets.

## Hosted qualification and receipts

`.github/workflows/operational-qualification.yml` runs the harness against a named
release on hosted runners. It is `workflow_dispatch` only, with `release_tag`
required and `prior_release_tag` / `release_id` optional, and
`permissions: contents: read` at the top level.

Its header states the boundary it must not cross: it never rebuilds a release
artifact, never stages, publishes, or mutates a tag, contains no second producer
release matrix, uses only immutable action revisions, and takes `contents: read`
plus artifact-upload-only `contents: read` for receipts.

Three job shapes:

| Job | Targets | Stages |
| --- | --- | --- |
| `installed-execution` | `x86_64-unknown-linux-gnu` / `ubuntu-latest`, `aarch64-apple-darwin` / `macos-14`, `x86_64-apple-darwin` / `macos-15-intel` | install → fixture → installer → installed execution |
| `native-service` | per-platform service matrix | install → fixture → service → update/rollback |
| `windows` | `windows-latest` / `x86_64-pc-windows-msvc` | install → fixture → installed execution → service disposition |

Receipts are uploaded with the same pinned `actions/upload-artifact@ea165f8d…`
revision, named `installed-execution-<platform>-<target>-<run_id>-<run_attempt>`
and `native-service-…`.

**What a receipt proves:** that on that hosted host, at that run, the exact bytes
of that release — verified twice against that release's own manifest and
sidecars — installed, started, accepted an authenticated loopback execution, and
admitted or refused capabilities as claimed, and that a bounded JSON record of
those facts exists.

**What a receipt does not prove:** that any unlisted platform or configuration
behaves the same; that the release is published, or will be; that behaviour is
stable on real hardware, real service managers with real privileges, or a
non-loopback network; that the next run will pass; or that the product is fit for
purpose. It is evidence about one candidate on one hosted host — which is exactly
why the published disposition for Windows SCM is `unsupported` rather than
"untested".

## Windows determinism and the PDB tradeoff

`.cargo/config.toml` is Eggwork product build policy, and it is deliberately
narrow:

```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "link-arg=/BREPRO", "-C", "link-arg=/DEBUG:NONE"]
```

- **`/BREPRO`** — MSVC `link.exe` stamps the PE COFF header with wall-clock time
  unless this is given, so two builds minutes apart differ in bytes alone.
  `/BREPRO` derives the timestamp deterministically from the input.
- **`/DEBUG:NONE`** — debug information is emitted into a CodeView (RSDS) record
  whose GUID and age come from a freshly generated PDB, so the record differs per
  build even when the code is identical. Suppressing the record removes the
  second source of divergence.

Both are needed; either alone is insufficient.

The constraints are as important as the flags:

- they are scoped to `x86_64-pc-windows-msvc` only, with no `[build]` or
  `cfg(target_os = "windows")` table — Linux and macOS are byte-stable under
  their own toolchains and must not inherit MSVC link arguments;
- no workflow or environment may set `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, or
  `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS`. Cargo resolves the environment
  *instead of* target-scoped `target.<triple>.rustflags`, so such a setting would
  silently replace this policy rather than extend it. A test requires all four
  workflows — `ci.yml`, `release.yml`, `windows-reproducibility.yml`,
  `operational-qualification.yml` — to exist and be free of them, so deleting a
  workflow cannot bypass the guard;
- the five-target artifact contract and Eggpack's same-name digest / no-clobber
  behaviour are unchanged.

**Release Windows binaries intentionally carry no PDB** and no CodeView debug
directory. Symbols for a shipped release are therefore unavailable from the
release artifact itself; crash triage of a released Windows build relies on a
**local rebuild from the same revision**. This is accepted deliberately: a
release that cannot be reproduced byte-for-byte cannot be reused, verified, or
re-staged across a same-tag rerun, which is a stronger distribution property
than embedded release symbols. The tradeoff is documented in the config file
itself, not left implicit.

`.github/workflows/windows-reproducibility.yml` produces the native two-build
byte-identity proof **before any release tag is created**: it builds the release
Windows daemon twice in independent target directories on the pinned toolchain,
requires byte-identical SHA-256, and requires no CodeView record in either image
(`scripts/verify_windows_reproducibility.py`). See
[ci-guardrails.md](ci-guardrails.md) for the broader build-policy guardrails, and
`tests/release/test_windows_reproducibility.py` for the PE parser's fail-closed
behaviour — unsupported optional-header magic, debug directories outside file
data, and non-RSDS CodeView variants are all rejected, not tolerated.

## Real failure modes on record

Every guard in this document traces to something that actually happened. The
sources are `git log`, `.cargo/config.toml`'s own header, and
`plans/closure/operations-distribution/004-status.md` §9 and §24.

### `v0.1.0` — one source revision, two different Windows binaries

**What happened.** Historical draft `400502116`, source `8827ed5`. The same-tag
rerun `36868105194` built the identical source revision and produced
`eggwork-v0.1.0-x86_64-pc-windows-msvc.exe` with a different SHA-256 than the
staged asset. PE inspection showed identical section sizes and image size, but a
different COFF timestamp and a different CodeView/PDB RSDS signature
(`19ab1c9c…` staged versus `cd86a174…` in the rerun).

**What it proved.** Eggpack failed closed on the same-name digest mismatch
instead of replacing the asset — the producer behaved correctly and the defect was
Eggwork's.

**The fix and the guard.** `/BREPRO` and `/DEBUG:NONE` in `.cargo/config.toml`,
guarded by `the_windows_msvc_build_policy_is_deterministic_and_target_scoped`
and `no_workflow_or_environment_can_replace_the_target_scoped_rustflags`, and
proven natively by `windows-reproducibility.yml` before any tag exists.

### `v0.1.3` — a bump that never regenerated the lock

**What happened.** Commit `d5213f7` bumped the workspace to `0.1.3`, changing
`Cargo.toml` without regenerating `Cargo.lock`. Every `--locked` release build
failed closed with "the lock file needs to be updated" (producer run
`37176445644`). The tag was left exactly as it is. Critically, **CI ran clippy and
test without `--locked` and therefore passed the gate that exists to prevent
exactly this.**

**The fix.** Commit `b31f117` — "fix(ci): require `--locked`, and bring
`Cargo.lock` up to the workspace version." Both now pass `--locked`.

**The guard.** `the_release_version_is_one_coherent_workspace_identity` requires
`Cargo.lock` to pin all five workspace members to `[workspace.package].version`,
and `scripts/check_release_tag.py` re-checks it before a tag exists.

### `v0.1.4` — staged binaries that reported the previous version

**What happened.** Commit `b31f117` missed the version bump entirely. The tag
named `0.1.4`; the staged binaries reported `0.1.3`. Hosted qualification run
`37185169008` failed **every** install stage with
`installed-daemon-version: daemon reports '0.1.3', expected 0.1.4`.

**Why that is a success.** That refusal is the version-coherence check working as
designed. The failure is real process evidence, not a harness problem.

**Staging itself was sound.** Run `37178170560` succeeded with 17 assets,
`draft=true`, `published_at=null`; the same-tag rerun `37183381984` succeeded with
`created=false, uploaded=0, reused=17`; the manifest cross-checked 17/17 with
`source_revision` equal to the tag source. That rerun's `reused=17` is the
same-name no-clobber rule working: nothing was replaced, nothing was uploaded.

**The fix.** Commit `4a6e7f5` — "fix(release): bump to 0.1.5 and fail fast when
the tag and version disagree" — plus `c3ece0c`, "fix(release): keep the generated
workflow generated; guard the tag locally."

**The guard.** `scripts/check_release_tag.py` runs before the tag exists and fails
if the workspace version or any locked member disagrees with the tag. The
commit subject is explicit about *where* the check must live: the tag-to-version
link cannot live in CI, because the release workflow is Eggpack producer authority
and the `release-drift` job rejects hand edits to it — and CI never knows the
future tag. So the check runs at tag time, by whoever cuts the tag. A second
guard is the producer's own version-coherence check plus the static-parity test,
both of which reject a partial bump.

### The generated workflow drifting from the producer

**What happened.** Hand-editing the generated workflow is always a possible
defect; the record commits to the opposite failure too — regenerating it wrongly.

**The fix.** `c3ece0c` — "fix(release): keep the generated workflow generated;
guard the tag locally." The tag guard moved out of the workflow and into
`check_release_tag.py`, and the workflow is derived only from
`workflow-shape.json`.

**The guard.** `eggpack ci check` in ordinary CI fails on drift, and
`the_generated_workflow_is_derived_only_from_static_configuration` fails if the
rendered workflow contains the current version, a sample version, an all-zeros
source revision, or the static runtime plan path.

### Windows service qualification — error 1053

**What happened.** Hosted runs `37095687727` and `37096444794`:
`QUALIFICATION FAILED (service): eggworkd.exe exited 2: eggworkd: service manager
failed: start service failed in Windows SCM (code 1053)`. Error 1053 is "the
service did not respond to the start request in a timely fashion".

**The cause, confirmed by inspection rather than inferred.** `eggworkd`
implements **no service-control dispatcher**. Grepping the workspace found no
`windows_service::service_dispatcher` or equivalent in `crates/`; the
`windows-service` crate reaches the dependency graph only as a transitive
dependency of `eggup-service`'s manager side. The process the SCM launches
therefore never connects back to the manager, so the registration can never reach
`Running`. The typed adapter registering the service *successfully* is what made
this look like flakiness.

**The disposition (landed in `caa3918`).** Every mutating `service` verb on
Windows is refused before any mutation, with a message naming the unsupported
backend. The recorded SCM product policy and typed adapter are retained in
`deployment::windows_scm_manager` (marked `#[allow(dead_code)]`) for a future
service-host milestone; only the operator surface is closed. `deployment status`
still reports host facts, and binary installation and daemon runtime are
unaffected.

**Why refuse rather than leave "untested."** Registering an external process would
leave a working installation with **no service behind it**, which is worse than an
honest refusal. Registering the daemon as a Windows service host is a product
capability, not a qualification fix, so it is a separate milestone.

**The guard.** The candidate's bytes predate the change, so the candidate
registers-and-fails-1053 while `main` fails closed before mutating. The
qualification workflow records **both** observations and fails if either changes —
the `windows-platform-disposition` gate. A second real bug in the same area: the
Windows disposition step was inheriting the daemon's exit code (`32e4559`), which
would have silently turned a recorded disposition into a job failure.

### A Windows-shaped run failure attributed to the wrong cause

`v0.1.2` reproduced the same `Internal`/`null` shape and was attributed to a
**third** defect — an output-monitor race — on the theory that two earlier fixes
had finally let the code path be reached. **That attribution was wrong.** `v0.1.5`
contains the monitor fix and reproduces the identical shape on hosted Windows (run
`37190674127`) while every Unix target passes. The actual cause predates all the
fixes: `LocalProcessRunner::run` refuses with `UnsupportedPlatform` on
`not(unix)` before any child exists. The monitor race is real and now pinned by a
test, but it was never the Windows cause.

Recorded here because the *process* failure is the durable lesson: a hypothesis
that "explains" a symptom is not evidence, and the record was corrected rather than
quietly rewritten.

### Windows CI-invisible dependency gating

Run `36784830178`: `landlock` is Linux-only, and its unconditional declaration
broke the macOS and Windows daemon builds. Every required non-Linux release
target builds `eggwork-server`, so this is release-blocking on Linux CI.

**The guard.** `linux_only_dependencies_never_enter_the_non_linux_build_graph`
requires `landlock` (runner, helper) and `nix` (helper) to be gated behind exactly
one platform table each, and asserts the table names the right platform.

### The release API having a bad day

Hosted qualification aborted on a single well-formed API request answered with a
5xx — indistinguishable from a real product failure, which would make the
qualification record depend on upstream uptime rather than the candidate's
behaviour. Commit `49200d8` — "fix(qualification): retry transient release API
failures, and fix the Windows job." Only 5xx and 429 are retried; a 4xx is a
harness bug and retrying it would hide that behind a delay.

## Invariants and enforcement

| Invariant | Enforcement site | Test / workflow |
|---|---|---|
| Exactly five canonical targets published, aliases resolve to their own triple | `release/eggpack/distribution.toml` | `contract_publishes_exactly_the_five_canonical_targets`, `every_contract_alias_resolves_to_its_own_triple` |
| Linux = one two-member bundle; macOS/Windows = daemon only; Windows `.exe` | `distribution.toml`, `build-bindings.toml` | `linux_targets_publish_one_two_member_bundle`, `non_linux_targets_publish_the_daemon_directly`, `windows_publishes_an_exe_suffixed_executable` |
| Every release file has an exact `.sha256` sidecar; names unique case-insensitively | `distribution.toml` | `every_release_file_has_an_exact_sha256_sidecar`, `expanded_release_names_are_unique_under_case_insensitive_comparison` |
| Install identities match `deployment::install_unit_matrix` | producer contract ↔ consumer matrix | `install_identities_match_the_deployment_install_unit` |
| Daemon and helper share one release identity; helper only on Linux | bundle form in `distribution.toml` | `the_linux_helper_and_daemon_share_one_release_identity`, `the_helper_ships_exactly_on_linux_release_targets`, `the_daemon_ships_on_every_release_target` |
| No release asset carries node state | `distribution.toml` | `no_release_asset_carries_node_state` |
| Every target built and qualified natively; toolchain matches `rust-toolchain.toml` | `release/eggpack/pack.toml` | `pack_policy_builds_and_qualifies_every_target_natively`, `toolchain_policy_matches_the_checked_in_rust_toolchain` |
| Build bindings name existing packages and binaries | `build-bindings.toml` | `build_bindings_name_exact_packages_and_binaries`, `build_binding_packages_and_binaries_exist_in_this_workspace` |
| Core smoke always selects the daemon with a fixed argv; selector names a bound output | `qualification-bindings.toml` | `core_qualification_smoke_selects_the_daemon_with_a_fixed_argv`, `the_qualification_selector_always_names_a_bound_build_output` |
| Consumer validation is gating, selects the Linux helper, agrees with deployment version policy | `consumer-validators.json` | `consumer_validation_is_gating_and_selects_the_linux_helper`, `the_linux_helper_validator_agrees_with_deployment_version_policy` |
| Bundle members install executable; nothing else declared | `install-policy.toml` | `install_policy_marks_every_bundle_member_executable_and_nothing_else` |
| No tag, SHA, digest, or size committed under `release/eggpack/` | producer config (by construction) | `static_release_configuration_embeds_no_future_release_identity` |
| `workflow-shape.json` matches the checked-in configuration | `workflow-shape.json` | `the_embedded_workflow_shape_matches_the_checked_in_configuration` |
| Runners, actions, and the Eggpack tool are pinned immutably | `github-policy.json` | `github_policy_pins_every_runner_action_and_the_tool_immutably`, `the_generated_workflow_pins_actions_and_installs_the_pinned_tool` |
| The release workflow is generated, never hand-maintained | `eggpack ci generate` / `ci check` | `the_generated_workflow_is_derived_only_from_static_configuration` |
| `contents: write` only in the staging job | `github-policy.json` staging | `the_generated_workflow_grants_contents_write_to_only_the_staging_job` |
| Staging creates/moves/publishes no tag and clobbers nothing | `workflow-shape.json` staging, `github-template.json` | `the_generated_workflow_never_publishes_or_mutates_tags`, `the_github_draft_policy_is_draft_only_and_needs_no_wrapper` |
| One runtime identity, resolved once, re-verified by every job | `github-policy.json` `release_inputs` | `the_generated_workflow_propagates_one_runtime_identity_to_every_job` (18 jobs) |
| No second hand-maintained release matrix | directory-level policy | `no_second_hand_maintained_release_matrix_exists` |
| One coherent workspace version across manifest, lock, and binaries | `Cargo.toml` + `Cargo.lock` | `the_release_version_is_one_coherent_workspace_identity` |
| Tag, version, and lock agree before the tag exists | `scripts/check_release_tag.py` | pre-tag guard, run by whoever cuts the tag |
| `eggwork-server` never parses producer configuration | consumer/product code | `the_deployment_module_never_parses_eggpack_configuration` |
| Linux-only deps never enter the non-Linux build graph | crate manifests | `linux_only_dependencies_never_enter_the_non_linux_build_graph` |
| Windows MSVC build is byte-deterministic and target-scoped | `.cargo/config.toml` | `the_windows_msvc_build_policy_is_deterministic_and_target_scoped` |
| No workflow/environment can replace target-scoped rustflags | workflow corpus | `no_workflow_or_environment_can_replace_the_target_scoped_rustflags` |
| Windows byte-identity before any tag exists | `.cargo/config.toml` | `.github/workflows/windows-reproducibility.yml` + `tests/release/test_windows_reproducibility.py` |
| Every retrieved artifact matches that release's sidecar **and** manifest | `scripts/qualify_release.py` | `tests/release/test_qualify_release_harness.py` |
| Only the exact requested tag is qualified | `qualify_release.py` `_resolve_release` / `fetch_release` | `test_a_draft_read_still_requires_a_token` |
| A tokened read cannot pose as public evidence | `qualify_release.py` `PUBLIC_MODE` | `test_a_tokened_read_cannot_masquerade_as_public` |
| Candidate validation is bounded, offline, shell-free, output-free | `scripts/release_candidate_probe.py` | `test_probe_never_shells_out`, `test_probe_never_reaches_the_network`, `test_candidate_environment_is_an_allowlist`, `test_entrypoint_reports_a_fixed_reason_without_candidate_output` |
| Installed binary admits/denies truthfully and drains | `installed_qualification.rs` | `.github/workflows/operational-qualification.yml` |
| Qualification is opt-in, never in the shipped daemon | `qualification` feature | `#![cfg(feature = "qualification")]` |

## Test coverage and gaps

**`tests/release/` tests the harness, not the product.** Three modules:

- `test_qualify_release_harness.py` — the harness's API behaviour: an asset is
  written, transient server errors, rate limits, and transport errors are retried,
  a client error is *not* retried, exhausted retries fail loudly, an oversized
  asset is refused without retrying, and the public/token split behaves.
- `test_release_candidate_probe.py` — the validators, against fabricated
  candidates: the helper accepts the workspace version and rejects a different one,
  an empty line, more than one line, the daemon as a substitute, a non-zero exit,
  and non-UTF-8 output; the daemon rejects a missing `version` field, the helper as
  a substitute, and non-JSON output; an unusable or symlinked candidate is refused;
  the entrypoint takes exactly one argument and reports a fixed reason; the
  workspace version fails closed on a malformed manifest, a missing package
  version, and an out-of-bounds value; the probe never shells out, never reaches
  the network, and its environment is an allowlist.
- `test_windows_reproducibility.py` — the PE parser reads the COFF timestamp the
  historical mismatch turned on, finds the RSDS record a debug build carries, and
  fails closed on non-RSDS CodeView variants, unparseable input, unsupported
  optional-header magic, and debug directories outside file data; plus the
  preflight comparison logic, including rejecting an environment `RUSTFLAGS`
  override.

**`release_contract.rs`** is the only suite that runs in ordinary `cargo test` and
therefore the real local gate. It covers the static producer configuration
completely: contract, parity with the consumer, policy, identity-freedom, the
rendered workflow, build determinism, and the sealed runtime boundary. It needs no
network, no runner, and no installed release.

**`installed_qualification.rs`** is `#[ignore]`d and opt-in. It never runs in an
ordinary test run; it runs only from the hosted workflow, or by hand with an
installed release and a fixture in hand.

**Honest gaps:**

- **Hosted-runner-only behaviour is not reproducible locally.** The service-lifecycle
  matrix (systemd user unit, launchd `LaunchAgents`, Windows SCM disposition),
  the native `update`/rollback path against real generations, and the
  three-platform receipt set can only be produced on those runners. A local run of
  `qualify_release.py` proves the same stages on one host; it cannot produce the
  matrix.
- **No local reproduction of byte-identical Windows builds.** The two-build proof
  is native-MSVC-only. `tests/release/test_windows_reproducibility.py` tests the
  *comparator* with synthetic inputs, not real link output. The property itself is
  established by the hosted workflow, not locally.
- **The end-to-end run — build, stage, qualify, publish — has no automated
  coverage.** Staging is a manual dispatch against an exact existing tag, and
  publication is a human action. The negative paths (a moved tag, a clobbered
  asset, a version mismatch) are proven by recorded real failures and by unit tests
  of the guard logic, not by a rehearsed full pipeline.
- **Receipt trustworthiness is not machine-checked.** Receipts are uploaded
  artifacts. Nothing in the repository asserts that a receipt's claims match a
  release, and `stage_inventory` is the one place a consumer-shaped read exists —
  a receipt is evidence a reviewer reads, not an input a test consumes.
- **Validator coverage of its own edge paths is thin.** `run_candidate`'s
  OSError/timeout branches and `decode_bounded`'s control-byte rejection are
  asserted in the probe tests for some inputs, but the validators' interaction with
  Eggpack's actual handoff (argument passing, cwd, exit-status recording) is
  verified only by the producer run.
- **The `check_release_tag.py` pre-tag guard is manual by design.** It runs
  locally, by a person, immediately before a tag. Nothing prevents someone from
  tagging without running it — the producer's version-coherence check is the
  backstop that makes that recoverable rather than shippable.
- **Five targets, three qualification platforms for execution.** Windows installed
  execution and service disposition run, but the deepest Unix evidence
  (required-isolation enforcement with a side-effect marker) is Linux-only by
  construction, since only Linux ships the Landlock helper.

## Review focus

- **Parity-test completeness.** Does `release_contract.rs` cover every field it
  reads, or are there contract keys no test inspects — in particular, is every key
  in `github-policy.json` and `github-template.json` asserted by some test, or
  merely parsed?
- **Can a version bump land without a lock update?** `check_release_tag.py` and
  `the_release_version_is_one_coherent_workspace_identity` cover the tag and the
  five members; `--locked` covers the build. Is there any path — a new crate, a
  `[workspace.dependencies]` change — that could still let a member's locked
  version drift?
- **Draft clobber protection.** `--clobber` is forbidden and Eggpack refuses a
  differing same-name asset. But is the *permissive* case examined: does the
  recorded same-tag rerun with `reused=17` prove identical bytes are reused
  without a re-upload, or only that nothing was clobbered?
- **Token scoping.** `RELEASE_QUALIFICATION_TOKEN` is required because drafts are
  invisible to `GITHUB_TOKEN`. Is the required scope narrow enough? The harness
  fails closed on a token in `--public` mode — is every other emission path
  (receipt, error, `log()`) also checked for serialisation?
- **First-install overwrite refusal.** Is the "never overwrite" claim in
  `install-policy.toml` enforced by the *generated installer* (Eggpack
  authority), making the `installer` stage the only observation point — and is
  that stage run on every platform?
- **Receipt trustworthiness.** Can a stage `record` a fact it did not verify, and
  is the `expect`/`record` distinction relied on everywhere it should be? Does
  `stage_inventory` output ever get cross-checked against a manifest?
- **Untested validator paths.** `run_candidate`'s symlink / non-regular-file
  refusal, the `MAX_VERSION_BYTES` bound, and the 30 s timeout are the paths most
  likely to be wrong. Do the probe tests exercise each, or only
  happy/one-bad-input paths?
- **Could a candidate smuggle a second release matrix?** The test scans
  `.github/workflows/` for `--target '<triple>'` outside `release.yml`. Would a
  matrix spelled with YAML `matrix:` blocks, or in
  `operational-qualification.yml` under different spellings, be caught? That
  workflow legitimately names targets — does the guard's intent (no second
  *producer* matrix) match what it forbids?
- **`SOURCE_VERIFYING_JOBS = 18` is hardcoded.** An excellent tripwire, but a job
  added together with its own bump of the constant would pass. Is the count's
  derivation from `workflow-shape.json` asserted anywhere?
- **The PDB tradeoff is a real, accepted cost.** Release Windows binaries have no
  symbols. Is local-rebuild-from-the-same-revision triage actually practical — do
  the pinned toolchain and build flags reproduce what a triager needs?
- **The 1053 disposition is time-sensitive.** Every mutating Windows `service`
  verb is refused, but `deployment::windows_scm_manager` is retained
  `#[allow(dead_code)]` for a future milestone. Does anything prevent that code
  from quietly becoming reachable again?
- **The fifth authority.** Is anything in the producer configuration trying to
  override `.cargo/config.toml`, and would a future `pack.toml` change (say,
  adopting CargoZigbuild) silently interact with the MSVC flags?

## Related

- [overview.md](overview.md) — system-level orientation
- [distribution.md](distribution.md) — producer/consumer ownership split and the
  full four-authority analysis
- [deployment-lifecycle.md](deployment-lifecycle.md) — the consumer installation
  transaction, service lifecycle, drain/update/rollback policy
- [operations-cli.md](operations-cli.md) — the operator command surface
- [ci-guardrails.md](ci-guardrails.md) — CI build-policy guardrails and the
  two-build Windows byte-identity proof
- [sandbox-helper.md](sandbox-helper.md) — the Landlock helper and its version
  policy
- [runner-execution.md](runner-execution.md) — execution paths and platform support
- [README.md](../README.md) — normative product behavior: "Releases and
  installation", "Reproducible Windows release binaries", "Qualifying an
  installation"
- [CONTRIBUTING.md](../CONTRIBUTING.md) — contributor workflow and gates
- `plans/closure/operations-distribution/003-status.md` — M003 closure record:
  producer configuration, the pinned tool, first-install bootstrap
- `plans/closure/operations-distribution/004-status.md` — M004 closure record:
  installed execution, the platform service matrix, the Windows 1053 disposition
  (§9), and the release lineage table (§24)
- [plans/registry.md](../plans/registry.md) — current ready/blocked work and
  external interface baselines
