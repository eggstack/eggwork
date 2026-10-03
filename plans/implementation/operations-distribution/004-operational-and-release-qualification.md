# Operations and Distribution M004 — Operational and Release Qualification

Status: **conditionally closed** — implementation substantially complete; three
named non-critical evidence items outstanding (public publication/bootstrap,
Windows service management, Windows child execution). Closure record:
`plans/closure/operations-distribution/004-status.md`. Acceptance criterion 5
is unmet for Windows by the §15 `unsupported` disposition rather than by
omission; the plan's own §15 anticipates unsupported backends.

Reviewed Eggwork baseline:

- Operations M002a implementation/closure baseline `c33e9d6fded7a80a377d6e6748cc9cdb2b02acbc`;
- Operations M004 implementation `b760c1a1705bcb2bcb894b78205fbc3befe2211d`;
- runner output-monitor corrective `68a6cc2b1308060156e3d7af7a94bd61665fbf9c`;
- latest qualification-evidence head before this rebaseline `1fcd426a4740b5f1018e5b6e1ca4988e023e9f55`;
- Operations M003 closure `3482e3655d590efe056fefff955317d50ab7b584`;
- historical release run `36787942079`;
- historical same-tag rerun `36868105194` (all producer/qualification jobs
  succeeded; staging failed closed on the Windows digest mismatch).

Historical release evidence:

- annotated tag `v0.1.0` -> `8827ed4812bca3c5a749182e09208e3695549f54`;
- GitHub release id `400502116`;
- state: draft, unpublished;
- staged inventory: 17 assets;
- all five producer targets built/qualified in M003;
- the exact staged Linux daemon exposes the output-monitor warning corrected
  only in source at `68a6cc2`.

The `v0.1.0` draft is immutable discovery/evidence and is no longer the
successful M004 qualification candidate.

Corrected qualification release:

- use the next patch release `v0.1.1` (verified unused during this planning
  pass);
- bump `[workspace.package].version` and the workspace lockfile package
  versions to `0.1.1` before tag creation;
- require daemon/helper version output, Eggpack release id, manifest, sidecars,
  and asset names to agree on `0.1.1`;
- create the annotated tag only after repository gates and Windows
  reproducibility preflight pass;
- stage a new draft; never move `v0.1.0`, modify release id `400502116`,
  or replace historical assets.

External baselines:

- published Eggup `v0.1.1` / `881c95ff069d3d465a282cb6a495ba6fcb70cb6f`;
- current Eggup main reviewed at
  `0f791324be406b9a07dd35c1c1121279bc38dcdb`;
- Eggpack producer pin now checked into Eggwork:
  `32a0903936fcc283863e0bfb86151b13b4d75ce9`;
- Eggpack current main reviewed during this rebaseline:
  `56ed7e747fd39e4d6a32a9f1fe3e09dd44355069`; Eggwork intentionally remains
  pinned to qualified durable revision `32a0903936fcc283863e0bfb86151b13b4d75ce9`.

Source roadmap:

- `plans/subsystems/operations-distribution-roadmap.md`

Satisfied prerequisite:

- Operations M002a is closed at
  `plans/closure/operations-distribution/002a-status.md` with no unresolved
  high/medium finding.

Primary class: operational qualification / platform lifecycle / release closure

## 1. Objective

Close the Operations/Distribution workstream by proving that the M003 producer
contract can emit a corrected, deterministic release candidate that behaves as
installable, runnable, updateable node software on the supported operating
systems without importing producer, scheduler, or platform-manager authority
into Eggwork.

M004 must establish evidence for:

- exact-release first installation from the corrected release assets produced through the M003 contract;
- installed daemon/helper version and digest coherence;
- product-owned service definition/policy on Linux, macOS, and Windows while
  all manager mechanics remain Eggup-owned;
- hosted service install/status/start/restart/stop/uninstall behavior on each
  platform actually claimed;
- real installed Eggwork execution behavior and truthful capability admission;
- drain + update + rollback/recovery behavior through the corrected M002a
  lifecycle composition;
- same-tag release rerun/reuse/no-clobber behavior;
- final draft/publication boundary and post-publication installer evidence;
- a truthful support matrix for Linux, macOS, and Windows.

M004 does not add scheduling, automatic release discovery, a self-updater,
credentialed auto-publication, or a new sandbox backend.

## 2. Current qualification blockers and rebaseline

M002a is closed and the principal M004 implementation has landed. The first
live qualification pass produced evidence that changes the release target but
not the architecture.

### 2.1 Historical `v0.1.0` cannot close M004

The exact staged Linux `v0.1.0` daemon completed a real mTLS execution but
reported `cleanup_warning: "output monitor closed"`. Current source fixes the
race at `68a6cc2`, and current-source mTLS smoke completes without the
warning. Because the existing draft is immutable evidence from the old source
revision, it cannot be repaired in place.

M004 must therefore qualify a new patch release rather than trying to make
`v0.1.0` pass.

### 2.2 Windows reproducibility is Eggwork-owned

The failed `v0.1.0` rerun showed the same MSVC nondeterminism independently
found by eggsact: wall-clock PE timestamps plus CodeView/PDB identity make a
fixed-source Windows binary change between builds.

eggsact commit `f1352101dab748066c788e65e21e1bf303cfe995` demonstrates a
product-side correction using target-scoped linker flags:

- `/BREPRO`;
- `/DEBUG:NONE`.

That implementation is useful evidence, but it is **not an upstream Eggwork
dependency**. Eggwork has no `.cargo/config.toml` today and must adopt and
qualify its own deterministic Windows policy. Eggpack's fail-closed
same-name/digest behavior remains unchanged.

### 2.3 Native service claims need proof or a fail-closed disposition

macOS launchd and Windows SCM policy/adapter code is implemented, but adapter
unit tests are not native manager qualification. For each backend M004 must
choose one truthful closure path:

1. obtain hosted native lifecycle evidence and mark the service backend
   qualified; or
2. keep/restore a structured fail-closed mutation refusal for that backend and
   mark service management unsupported/unqualified while separately qualifying
   daemon/runtime support.

M004 must not close with an exposed destructive service path described only as
"implemented but untested."

### 2.4 Remaining evidence

M004 still needs:

- deterministic Windows candidate proof in Eggwork;
- a corrected `v0.1.1` five-target draft;
- byte-identical same-tag rerun/reuse on that corrected draft;
- installed runtime qualification from corrected release bytes;
- required Linux installed-helper isolation evidence or an explicit unsupported
  disposition;
- live update/rollback/recovery/state-preservation evidence;
- native service-manager evidence for every backend left enabled;
- public bootstrap evidence only if publication is explicitly authorized.

Historical M002/M003/M002a evidence remains unchanged. Operations M005/M006
remain deferred.

## 3. Authority boundaries

### Eggwork owns

- product service identity and arguments;
- config path identity;
- platform-specific product definition bytes/settings;
- drain and active-execution policy;
- expected installed version/helper coherence checks;
- support claims and operator diagnostics;
- whether/when a human publishes a validated draft.

### Eggup owns

- service-manager inspection and mutation;
- ownership classification;
- transition deadlines;
- artifact transaction, backup, rollback, recovery evidence;
- lifecycle restoration around a validated transaction.

### Eggpack owns

- release target/artifact contract;
- build/qualification;
- checksum sidecars;
- ReleaseManifest;
- bootstrap generation;
- generated release workflow and draft staging.

M004 MUST NOT copy any of those mechanics into a parallel implementation.

## 4. Platform service policy

Remove the current blanket non-Linux mutation refusal only for paths that have
an Eggwork-owned product policy plus hosted evidence.

Keep one common `ServiceSpec` identity:

- service id: `eggwork-node`;
- executable: exact absolute installed `eggworkd`;
- argv: `run --config <absolute-config>`;
- config identity: same exact absolute config path.

### 4.1 Linux — systemd

Retain the existing Eggup `SystemdManager` path.

Before live qualification, harden `render_systemd_unit` so it never emits an
ambiguous `ExecStart` for paths containing characters Eggwork does not encode
safely.

Either:

1. implement a reviewed systemd-argument encoder, or
2. reject unsupported whitespace/metacharacter path forms explicitly.

Do not continue accepting a path that renders a different argv than the
`ServiceSpec`.

Qualified product policy remains explicit:

- system or user scope selected by operator;
- explicit absolute unit path;
- caller-selected enable/reload flags;
- no automatic privilege escalation.

### 4.2 macOS — launchd

Add Eggwork-owned deterministic plist rendering and a constructor around
Eggup's published `LaunchdManager`.

Use:

- label `eggwork-node`;
- exact `ProgramArguments`:
  `<eggworkd> run --config <config>`;
- no shell command/string;
- XML escaping for every caller-derived value;
- no hidden environment injection;
- no implicit EUID/domain selection.

CLI/operator policy must make domain and target explicit:

- `user` -> Eggup `LaunchdDomain::UserAgent`, target `gui/<uid>`;
- `system` -> `LaunchdDomain::SystemDaemon`, target exactly `system`;
- explicit absolute plist path;
- explicit bootstrap-on-install choice.

Initial hosted qualification SHOULD use a user-agent domain when the runner
permits safe mutation without privilege escalation. System-daemon support is
not claimed merely because the renderer can construct it; it requires
privileged hosted evidence of its own.

Do not set a `KeepAlive`/restart policy that prevents an explicit stop from
converging. If product supervision semantics beyond Eggup's basic lifecycle are
desired, register a separate policy decision rather than improvising it here.

### 4.3 Windows — SCM

Add Eggwork-owned policy around the existing published
`WindowsScmManager::new`.

Initial descriptor:

- service name: `eggwork-node`;
- display name: `Eggwork Node Daemon`;
- start type: `Manual` by default, with any configurable alternative
  bounded to Eggup's finite `WindowsStartType`;
- error control: `Normal`;
- no custom account password;
- no implicit dependencies;
- bounded transition timeout.

All interaction must use Eggup's typed SCM adapter.

Forbidden:

- `sc.exe`;
- PowerShell service mutation;
- shell interpolation;
- registry editing;
- auto-UAC/elevation.

If the hosted Windows runner lacks service-manager authority, record the path
as unqualified rather than bypassing the ownership model.

## 5. CLI and machine-readable operator surface

Extend the existing `eggworkd service` command so one product command maps to
the native Eggup adapter for the current OS.

Do not auto-select from multiple discovered managers when destructive mutation
is requested. Manager policy must be explicit.

Recommended flags:

### Linux

Preserve:

- `--unit-path`;
- `--scope system|user`;
- `--enable`;
- `--no-reload`.

### macOS

Add:

- `--plist-path <absolute>`;
- `--launchd-domain user|system`;
- `--launchd-target <gui/uid|system>`;
- `--bootstrap-on-install` when desired.

### Windows

Add finite policy:

- `--windows-start-type manual|automatic|disabled` with `manual` default.

Every mutating result remains JSON and MUST include at least:

- schema version;
- service id;
- platform/backend;
- operation;
- completed status.

Status must report ownership/state where the selected adapter can prove it;
do not return only host availability.

## 6. Local-candidate deployment apply surface

M004 needs an executable operator path for the already-designed Eggwork/Eggup
update behavior. Add a bounded local-candidate command rather than inventing
release discovery.

Recommended shape:

```text
eggworkd deployment apply \
  --config <node-config> \
  --installation-root <absolute> \
  --release <opaque-release-id> \
  --daemon <absolute-local-candidate> \
  [--helper <absolute-local-candidate>] \
  --service-config <absolute> \
  <platform-manager options>
```

Rules:

- no URL;
- no `latest`;
- no GitHub API;
- no Eggpack runtime dependency;
- no manifest-driven acquisition;
- no automatic publication;
- candidates must already exist as regular local files.

Platform member policy:

- canonical Linux **release** deployment requires daemon + helper together,
  matching M003's two-member release bundle even when the current node config
  does not request Landlock today; this prevents a later helper enablement from
  discovering a silently stale generation;
- lower-level library fixtures may still exercise the historical optional-helper
  matrix where appropriate, but the M004 operator release path is coherent;
- macOS/Windows deployment accepts daemon only;
- unexpected helper on non-Linux is rejected rather than silently ignored.

Before invoking the corrected M002a lifecycle transaction, qualification
tooling MUST verify downloaded release bytes against the staged/published
ReleaseManifest and sidecars. Production `deployment apply` remains a
local-candidate operation and does not become a release-channel client.

Machine-readable output must expose:

- release id;
- Core artifact disposition;
- whether manual recovery is required;
- lifecycle restoration status;
- final observed ownership/state when available;
- drain remains active after update unless an explicit later operator action
  clears it.

No successful command may print `updated`/equivalent when the disposition is
`RecoveryRequired`.

## 7. Installed post-update check

M004 should consume the richer M002a lifecycle receipt and use an Eggwork-owned
bounded `PostInstallCheck`.

Minimum check:

- execute the installed daemon's `version` command through a bounded
  no-shell process invocation;
- require expected package version;
- on Linux with required helper, run the existing helper trust/version check
  against the installed helper;
- honor the remaining Eggup lifecycle deadline.

For qualification, an intentionally mismatched expected version MAY be used to
exercise the real rollback path. This is product policy input, not a hidden
fault-injection CLI.

Do not use remote network health as the sole post-install check: service
startup and protocol smoke are separate evidence below.

## 8. Installed execution qualification

M003 proved that each candidate can execute `eggworkd version`. M004 must
prove the installed node path.

For each canonical release target:

1. install the exact release artifact without a Rust toolchain;
2. verify expected destination files;
3. verify SHA-256 against the release evidence;
4. run `eggworkd version`;
5. run `eggworkd config validate` and `doctor` against a bounded
   qualification config;
6. start the node, using the native service manager where that path is claimed;
7. perform one authenticated loopback remote execution through the normal
   client/control-plane path;
8. verify terminal status and bounded output;
9. stop/cleanup.

Execution request policy:

- Linux: include a hosted smoke of required `workspace_rw` Landlock using the
  installed helper;
- macOS/Windows: use only capabilities actually advertised by the host;
- macOS/Windows must additionally prove a request requiring unsupported
  filesystem isolation is rejected as `capability_mismatch` before target
  spawn, using a marker/side-effect negative control.

Network Disabled/AllowListed remain unsupported unless a separate milestone
implements them; M004 must not weaken that behavior.

## 9. Hosted service-lifecycle matrix

Native service mutation is required for every service backend claimed as
supported.

Minimum matrix:

| Platform | Required live evidence |
|---|---|
| Linux | systemd install -> inspect Owned -> start -> running -> restart -> running -> stop -> stopped -> uninstall/cleanup |
| macOS | launchd user-agent install -> inspect Owned -> start -> running -> restart -> running -> stop -> stopped -> uninstall/cleanup |
| Windows | SCM create -> inspect Owned -> start -> running -> restart -> running -> stop -> stopped -> uninstall/cleanup |

Also test on every platform:

- foreign same-name registration is not mutated;
- malformed/unknown ownership is not mutated where a safe fixture can be
  constructed;
- repeated start/stop is idempotent;
- cleanup runs on both success and failed qualification.

Use isolated/ephemeral hosts or dedicated runners. A compile-only lane,
mock/fake adapter, or cross-build does not count as native service-manager
qualification.

No test may use automatic privilege elevation. Runner provisioning may grant
the job the authority it needs, but Eggwork itself must never obtain it.

If a native manager cannot be safely exercised on the available hosted
environment, do not leave the mutating CLI enabled merely because the Eggup
adapter exists. Gate that backend back to a structured unsupported/refusal
path, retain unit/renderer coverage, and record service management as
unsupported/unqualified for M004. Binary installation and daemon runtime may
still be qualified independently.

## 10. Update, rollback, and recovery qualification

After M002a, exercise the corrected lifecycle path with real installed
binaries.

Required semantic matrix:

1. running -> successful update -> running;
2. stopped -> successful update + Preserve -> stopped;
3. active executions + no force -> bounded refusal before replacement;
4. persistent drain prevents new acceptance;
5. failing post-install check + RollBack -> prior artifact generation restored
   and prior lifecycle state restored;
6. failing post-install check + KeepInstalled -> new generation retained with
   explicit lifecycle/check failure evidence;
7. `RecoveryRequired` -> service remains unstarted and manual recovery is
   explicit;
8. database/workspace/blob/artifact execution state remains outside the install
   transaction and byte/logically intact;
9. successful restart/recovery never fabricates interrupted non-idempotent work
   as completed.

A real distinct prior Eggwork build may be used for Linux update evidence if it
is recorded and executable. Platform tests that cannot safely manufacture a
second product generation may combine live service replacement with
deterministic M002a disposition tests, but MUST state that limitation rather
than claiming a full cross-version update.

## 10a. Eggwork Windows release determinism policy

Before creating the corrected release tag, add Eggwork-local deterministic
MSVC link policy.

Create `.cargo/config.toml` with a target-scoped
`[target.x86_64-pc-windows-msvc]` policy equivalent in effect to:

- `-C link-arg=/BREPRO`;
- `-C link-arg=/DEBUG:NONE`.

Requirements:

- the policy is scoped only to `x86_64-pc-windows-msvc`;
- no workflow may set `RUSTFLAGS` or equivalent in a way that replaces the
  target-scoped Cargo rustflags;
- the five-target artifact contract is unchanged;
- Eggpack digest/no-clobber behavior is unchanged;
- documentation records the deliberate tradeoff that release Windows binaries
  contain no PDB/CodeView debug directory.

Add a static regression guard that parses the Cargo config and requires both
link arguments.

Add native Windows reproducibility evidence **before staging**:

1. check out one exact source revision;
2. build the release Windows daemon twice in independent target directories on
   the same pinned toolchain;
3. compare SHA-256 byte-for-byte;
4. inspect the final PE and require no CodeView/RSDS record;
5. fail if workflow/environment configuration overrides target rustflags.

This proof is Eggwork-owned. eggsact M005a is prior art only.

## 11. Corrected draft and same-tag rerun/reuse gate

Historical rerun `36868105194` is already valid evidence: Eggpack reused the
`v0.1.0` draft, preserved all existing assets, and failed closed on the
nondeterministic Windows candidate. Do **not** rerun or repair that historical
draft as an M004 success criterion.

After the runner fix, deterministic Windows policy, repository gates, and
workspace version bump are present:

1. choose the exact reviewed source revision;
2. require workspace version `0.1.1`;
3. create annotated tag `v0.1.1` at that exact revision through explicit
   maintainer action;
4. dispatch the generated release workflow for exact `v0.1.1`;
5. require all five targets and validators green;
6. require a new draft with the expected 17-asset inventory;
7. leave it unpublished;
8. rerun the exact same tag with no source/tag/config changes.

Required rerun outcome:

- workflow succeeds end-to-end;
- staging receipt reuses the same `v0.1.1` draft;
- all 17 assets are byte-identical/reused;
- no differing same-name asset exists;
- no tag mutation;
- no duplicate release;
- no publication;
- no clobber path.

If any corrected-candidate asset differs, stop. Do not replace it. Diagnose
product/toolchain nondeterminism before continuing qualification.

## 12. Eggpack tool-pin durability

This prerequisite is now satisfied.

Eggwork pins Eggpack at durable main revision
`32a0903936fcc283863e0bfb86151b13b4d75ce9`, which contains the qualified
M003e/f/g workflow behavior plus M003h reconciliation. The generated
`.github/workflows/release.yml` was regenerated and `eggpack ci check`
reported zero drift.

Before the corrected release run, re-check that this exact revision remains
reachable and that no newer Eggpack revision is required by a concrete defect.
Do not churn the pin merely because main has advanced.

## 13. Corrected-release first-install bootstrap qualification

### Pre-publication

Use authenticated draft asset retrieval or workflow artifacts to inspect and
exercise the exact binaries.

Do not modify the generated exact-release installer solely to make a draft URL
publicly reachable.

### Post-publication

The corrected release's generated `install.sh` / `install.ps1` hardcode the exact
`releases/download/v0.1.1` origin. True public bootstrap qualification
therefore occurs only after an authorized human publishes the corrected draft.
Historical `v0.1.0` installers are not the M004 success target.

Required native smoke after publication:

- Linux x86-64 and AArch64: `install.sh` installs daemon+helper;
- macOS x86-64 and arm64: `install.sh` installs daemon only;
- Windows x86-64: `install.ps1` installs daemon only;
- destination-exists refusal works;
- installed bytes match sidecar/manifest;
- installed daemon version is `0.1.1`;
- Linux helper version matches and has executable mode.

No Cargo/Rust fallback is part of this release contract.

## 14. Publication authorization boundary

Implementation and CI MUST NOT publish the draft automatically.

Publication is an explicit maintainer action after all available
pre-publication M004 gates pass.

Before a maintainer publishes, record:

- exact corrected annotated tag/source (`v0.1.1` unless re-planned before tag creation);
- exact corrected draft release id;
- exact corrected 17-asset inventory and hashes;
- green same-tag rerun/reuse evidence;
- native service/installed execution qualification disposition;
- outstanding platform limitations;
- no high/medium unresolved finding.

If publication is not authorized during the M004 closure window, M004 may close
**conditionally** with public bootstrap/public-release smoke named as the sole
remaining release-publication evidence. Do not fabricate publication evidence.

If publication occurs:

- publish the corrected, fully qualified draft only;
- do not publish the historical `v0.1.0` draft as the M004-qualified release;
- do not move/recreate either tag;
- do not replace differing assets;
- verify the corrected public exact-tag asset inventory;
- run the post-publication bootstrap matrix.

M004 planning authorizes qualification work, not release publication.

## 15. Platform support disposition

Closure must publish a truthful matrix separating binary availability from
service and isolation support.

Expected categories:

- **release-qualified** — exact binary built and natively executed;
- **installed-runtime-qualified** — first install + daemon execution/control
  path exercised;
- **service-qualified** — native manager mutation exercised;
- **required-isolation-qualified** — required filesystem/process/resource
  capability actually enforced;
- **unsupported** — implementation/capability intentionally absent;
- **untested** — implementation may exist but no hosted evidence.

Do not infer one category from another.

Expected isolation baseline:

- Linux: Landlock `workspace_rw` may be qualified when installed-helper
  evidence passes;
- macOS: Landlock unavailable; required filesystem isolation remains
  unsupported unless a separate implementation lands;
- Windows: Landlock unavailable; required filesystem isolation remains
  unsupported unless a separate Windows isolation implementation is proven;
- network Disabled/AllowListed: unsupported on all platforms in current
  scope.

Unsupported required controls continue to reject before spawn.

## 16. Qualification workflow/harness

Add a product-owned manual operational-qualification workflow or equivalent
hosted harness when useful.

It may:

- download exact corrected draft/public assets;
- run first-install and service lifecycle;
- run installed execution smoke;
- collect bounded receipts/logs.

It MUST NOT:

- rebuild release artifacts;
- stage or publish releases;
- mutate tags;
- contain a second producer release matrix;
- use unpinned third-party actions;
- store secrets in artifacts/logs.

Keep producer drift enforcement in the existing generated release workflow and
`eggpack ci check`.

## 17. Documentation

Update, as applicable:

- `README.md`;
- `architecture/distribution.md`;
- operator/service usage text;
- platform capability documentation;
- Operations roadmap;
- registry.

Document:

- exact first-install vs update distinction;
- native service options per OS;
- no automatic elevation;
- local-candidate-only deployment apply semantics;
- manual recovery behavior;
- release draft/publication boundary;
- platform support matrix.

## 18. Verification

Repository gate:

```bash
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
git diff --check
```

Release gate:

- exact pinned Eggpack `ci generate`;
- exact pinned Eggpack `ci check`;
- Eggwork native Windows double-build reproducibility preflight;
- coherent workspace/package version bump to `0.1.1`;
- exact corrected `v0.1.1` draft build/qualification;
- same-tag `v0.1.1` rerun;
- staging receipt/all-asset reuse/no-clobber audit.

Platform gate:

- native binary/install smoke on all five release targets;
- native manager mutation on every service backend claimed;
- authenticated installed execution smoke;
- required-capability negative controls;
- update/lifecycle matrix after M002a.

MSRV remains Rust 1.89.

## 19. Acceptance criteria

1. Operations M002a is closed first.
2. No direct platform-manager implementation exists in Eggwork.
3. Linux systemd product policy is safe for all accepted paths.
4. macOS launchd product definition/policy is deterministic, ownership-safe,
   and live-qualified for every domain claimed.
5. Windows SCM product policy uses only Eggup's typed adapter and is
   live-qualified.
6. Clean supported hosts can install/run the release without Rust.
7. Every claimed service backend passes native install/start/restart/stop/
   uninstall evidence.
8. Installed remote execution succeeds on every platform claimed for runtime
   support.
9. Unsupported required isolation fails before spawn.
10. Drain/update/rollback/recovery behavior is truthful; `RecoveryRequired`
    never auto-starts.
11. Eggwork's Windows candidate is independently proven byte-reproducible, and the corrected release same-tag rerun reuses all 17 assets without clobber.
12. Release workflow/tool pins are immutable and their durability finding is
    resolved or explicitly low-severity.
13. Publication remains human-controlled.
14. If the corrected release is published, public exact-release bootstrap passes on all five targets;
    otherwise M004 closes conditionally with that named evidence outstanding.
15. Release/update/service ownership documentation matches production.
16. No unresolved high/medium correctness or security finding remains.

## 20. Stop conditions

Stop and register a corrective if:

- M002a does not close;
- Eggup 0.1.1 cannot express the required launchd/SCM product policy;
- hosted service mutation requires Eggwork-controlled privilege escalation;
- a platform can only pass by bypassing ownership checks;
- corrected-release rerun produces a differing same-name asset;
- a candidate can execute when a required unsupported capability was requested;
- update/recovery can auto-start after `RecoveryRequired`;
- the corrected release tag/source or staged draft inventory changes unexpectedly;
- publication would require tag movement or asset clobber;
- platform claims depend only on cross-compilation or mocks;
- a new high/medium finding appears.

## 21. Closure evidence

Create:

`plans/closure/operations-distribution/004-status.md`

Record:

- exact Eggwork implementation/qualification SHAs;
- M002a closure reference;
- exact Eggup dependency/tag disposition;
- exact Eggpack tool pin and durability status;
- historical `v0.1.0` tag/release evidence and its failure disposition;
- corrected `v0.1.1` tag + release id + asset inventory;
- corrected same-tag rerun staging receipt with all-asset reuse;
- native install matrix;
- service-manager matrix;
- installed execution/capability matrix;
- update/rollback/recovery matrix;
- persistent-drain and execution-truthfulness evidence;
- public publication/installer evidence, or explicit conditional outstanding
  publication evidence;
- final platform-support matrix;
- unresolved findings;
- Phase 6 exit disposition.

## 22. Phase transition

If M004 closes fully:

- Operations/Distribution M001-M004 are closed;
- Phase 6 exit criteria may be marked satisfied for the support matrix actually
  proven;
- reverse-connect M005 and PTY M006 remain deferred and do not block the first
  fixed-target release;
- downstream CodeGG M004 remains independently governed by CodeGG's AgentRun
  worker-entry contract.

Do not use M004 closure to claim capabilities that remain explicitly
unsupported on macOS/Windows.
