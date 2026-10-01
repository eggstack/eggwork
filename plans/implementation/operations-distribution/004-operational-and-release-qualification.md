# Operations and Distribution M004 — Operational and Release Qualification

Status: blocked

Reviewed Eggwork baseline:

- planning baseline `0d52435481c09e896fa376dd040fde92a5529685`;
- Operations M003 implementation `012383b4c19b7c598157381a29a3eec9bfaa5c1b`;
- hosted portability correctives `4b954438fbe5162947359af642441a957dc409c4`
  and `8827ed4812bca3c5a749182e09208e3695549f54`;
- Operations M003 closure `3482e3655d590efe056fefff955317d50ab7b584`;
- hosted release run `36787942079`.
- M002a implementation `c33e9d6fded7a80a377d6e6748cc9cdb2b02acbc`;
- M004 implementation `b760c1a1705bcb2bcb894b78205fbc3befe2211d`;
- same-tag rerun `36868105194` (producer/qualification success; fail-closed
  stage due the Windows same-name digest mismatch recorded in the closure).

Live release input:

- annotated tag `v0.1.0` -> `8827ed4812bca3c5a749182e09208e3695549f54`;
- GitHub release id `400502116`;
- state: draft, unpublished;
- staged inventory: 17 assets;
- all five producer targets built/qualified in M003.

External baselines:

- published Eggup `v0.1.1` / `881c95ff069d3d465a282cb6a495ba6fcb70cb6f`;
- current Eggup main reviewed at
  `0f791324be406b9a07dd35c1c1121279bc38dcdb`;
- Eggpack producer pin now checked into Eggwork:
  `32a0903936fcc283863e0bfb86151b13b4d75ce9`;
- Eggpack main reviewed during planning:
  `32a0903936fcc283863e0bfb86151b13b4d75ce9`; it contains qualified M003e/f/g
  behavior and M003h reconciliation.

Source roadmap:

- `plans/subsystems/operations-distribution-roadmap.md`

Hard prerequisite (satisfied):

- `plans/implementation/operations-distribution/002a-recovery-required-restart-suppression-and-lifecycle-composition-corrective.md`
  must close with no high/medium finding.

Primary class: operational qualification / platform lifecycle / release closure

## 1. Objective

Close the Operations/Distribution workstream by proving that the artifacts
already produced by M003 behave as installable, runnable, updateable node
software on the supported operating systems without importing producer,
scheduler, or platform-manager authority into Eggwork.

M004 must establish evidence for:

- exact-release first installation from the M003 release assets;
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

## 2. Why M004 is blocked today

M003 supplied the intended input: a real green five-target producer run and a
staged `v0.1.0` draft.

During M004 research, review of the already-closed M002 update wrapper found a
latent recovery defect: Eggwork can restart a previously running service after
an Eggup Core receipt whose disposition is `RecoveryRequired`. Published
`eggup-service 0.1.1` explicitly suppresses that restart through
`commit_with_lifecycle`.

M002a closed at implementation `c33e9d6`; that dependency is satisfied. M004
implementation landed at `b760c1a`. Linux x86-64 draft artifact integrity,
installed daemon/helper version execution, and native Linux user-systemd
install/start/restart/stop/uninstall have been exercised. Product policy now
includes strict systemd argument paths, deterministic launchd XML, typed
launchd/SCM adapters, manager-specific JSON status, and local-candidate apply.

Closure is blocked by qualification evidence:

1. Same-tag rerun run `36868105194` rebuilt every asset. Every artifact was
   byte-identical to the draft except
   `eggwork-v0.1.0-x86_64-pc-windows-msvc.exe`. The rerun artifact matches its
   own ReleaseManifest, but its PE COFF timestamp and embedded PDB signature
   differ from the staged executable. Eggpack main records exact rerun reuse as
   blocked by eggsact M005a Windows byte reproducibility. Eggpack failed closed
   before replacing any asset; the draft remains unchanged. Do not retry or
   clobber it until the upstream blocker is cleared.
2. The exact staged Linux x86-64 v0.1.0 daemon now has live mTLS execution,
   capability rejection, and persistent-drain evidence. That smoke exposed
   `cleanup_warning: "output monitor closed"` in the immutable release binary.
   A runner source correction and regression test remove the race in current
   source, but the staged artifact/tag do not contain the correction and need
   a new release input before exact-release qualification can pass. Native
   launchd and SCM lifecycle, required isolation on the claimed host matrix,
   and the complete update/rollback matrix also lack live evidence.
   Cross-compilation is not qualification.
3. The draft remains unpublished. Public bootstrap evidence requires an
   explicit maintainer publication action and remains outstanding.

No other registered Eggwork implementation plan becomes ready from this
partial qualification; Operations M005/M006 remain deferred. Historical M002
and M003 evidence is unchanged.

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

## 11. Existing M003 draft rerun/reuse gate

Before any public publication, rerun the generated M003 release workflow for
the exact existing annotated `v0.1.0` tag.

Preconditions:

- tag still resolves to
  `8827ed4812bca3c5a749182e09208e3695549f54`;
- existing release id remains `400502116`;
- release remains draft;
- existing asset inventory is exactly the recorded 17 assets;
- no asset has been manually replaced.

Required outcome:

- workflow succeeds;
- staging receipt says existing draft reused;
- byte-identical assets are reused, not clobbered;
- no tag mutation;
- no duplicate release;
- no publication;
- no differing same-name asset is overwritten.

If a rebuilt platform artifact differs, the workflow MUST fail closed. Diagnose
the producing toolchain/product determinism and register a corrective if the
difference is not expected and bounded. Never delete/replace the existing asset
merely to make the rerun green.

## 12. Eggpack tool-pin durability

M003 intentionally pinned
`8507fbeebc6e6a0f8176965d8b21dfc818a03719`, because the then-current Eggpack
main lacked the M003e/f/g generated-workflow fixes. During this planning pass
that revision is still reachable from
`refs/heads/m003g-live-qualification`, while Eggpack main is
`404f63ec...`.

At M004 implementation start:

1. re-check Eggpack main/tags;
2. if a durable main/tag revision contains the qualified M003e/f/g behavior,
   re-pin Eggwork to that exact immutable revision;
3. regenerate `.github/workflows/release.yml`;
4. run `eggpack ci check`;
5. repeat the producer/rerun gates affected by any generated-byte change.

Do NOT re-pin backward to a revision known to generate a broken hosted
workflow.

If no durable merged/tagged revision exists, the branch-only pin remains a
low-severity operational dependency. Record it explicitly; do not mask it.

## 13. First-install bootstrap qualification

### Pre-publication

Use authenticated draft asset retrieval or workflow artifacts to inspect and
exercise the exact binaries.

Do not modify the generated exact-release installer solely to make a draft URL
publicly reachable.

### Post-publication

The generated `install.sh` / `install.ps1` hardcode the exact
`releases/download/v0.1.0` origin. True public bootstrap qualification
therefore occurs only after an authorized human publishes the draft.

Required native smoke after publication:

- Linux x86-64 and AArch64: `install.sh` installs daemon+helper;
- macOS x86-64 and arm64: `install.sh` installs daemon only;
- Windows x86-64: `install.ps1` installs daemon only;
- destination-exists refusal works;
- installed bytes match sidecar/manifest;
- installed daemon version is `0.1.0`;
- Linux helper version matches and has executable mode.

No Cargo/Rust fallback is part of this release contract.

## 14. Publication authorization boundary

Implementation and CI MUST NOT publish the draft automatically.

Publication is an explicit maintainer action after all available
pre-publication M004 gates pass.

Before a maintainer publishes, record:

- exact annotated tag/source;
- exact draft release id;
- exact 17-asset inventory and hashes;
- green same-tag rerun/reuse evidence;
- native service/installed execution qualification disposition;
- outstanding platform limitations;
- no high/medium unresolved finding.

If publication is not authorized during the M004 closure window, M004 may close
**conditionally** with public bootstrap/public-release smoke named as the sole
remaining release-publication evidence. Do not fabricate publication evidence.

If publication occurs:

- publish the existing draft only;
- do not move/recreate `v0.1.0`;
- do not replace differing assets;
- verify the public exact-tag asset inventory;
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

- download exact existing draft/public assets;
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
- same-tag `v0.1.0` rerun;
- staging receipt/no-clobber audit.

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
11. Same-tag release rerun reuses the existing draft without clobber.
12. Release workflow/tool pins are immutable and their durability finding is
    resolved or explicitly low-severity.
13. Publication remains human-controlled.
14. If published, public exact-release bootstrap passes on all five targets;
    otherwise M004 closes conditionally with that named evidence outstanding.
15. Release/update/service ownership documentation matches production.
16. No unresolved high/medium correctness or security finding remains.

## 20. Stop conditions

Stop and register a corrective if:

- M002a does not close;
- Eggup 0.1.1 cannot express the required launchd/SCM product policy;
- hosted service mutation requires Eggwork-controlled privilege escalation;
- a platform can only pass by bypassing ownership checks;
- release rerun produces a differing same-name asset;
- a candidate can execute when a required unsupported capability was requested;
- update/recovery can auto-start after `RecoveryRequired`;
- the exact release tag/source or staged draft inventory changes unexpectedly;
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
- `v0.1.0` tag + release id + asset inventory;
- same-tag rerun staging receipt;
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
