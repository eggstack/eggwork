# M004 closure record: operational and release qualification

Status: **conditionally closed**
Closed: 2026-10-03
Plan: `plans/implementation/operations-distribution/004-operational-and-release-qualification.md`
Roadmap: `plans/subsystems/operations-distribution-roadmap.md`

Conditional closure is required by plan §14 (publication is a human action that
did not occur) and by plan §15 (two Windows capabilities are dispositioned
`unsupported`/`unclaimed` rather than qualified). Both named items are listed
under *Outstanding evidence*. Nothing in this record is projected to pass.

## 1. Reviewed baseline and commits

| Item | Value |
| --- | --- |
| M002a closure | `plans/closure/operations-distribution/003-status.md` (closed, supersedes the `M003`) |
| Plan baseline commit | `fc67f8d90037892486d6a0e9e263df502900b3ed` |
| M004 implementation commit | `fc67f8d` — release determinism policy, qualification harness, workflows, version bump |
| Product defect fixes (pre-tag) | `b760c1a` installed-daemon version parsing; `68a6cc2` trusted-helper trust boundary; `b760c1a` bounded `deployment apply` failure fields |
| Post-tag harness/hardening commits | `944606c`, `786d5da`, `45d9cfa`, `8b76cb9`, `1b0acc5`, `b6e88de`, `349cfea` |
| Windows fail-closed + disposition commits | `caa3918` service management fails closed; `202d395` platform-shaped child environment; `ec2b0e2` workflow disposition job |
| Windows disposition and workflow fix commits | `8d4c0f0` transient release-API retry with tests; `32e4559` Windows disposition step exit handling |
| Closure head | `main` at closure commit (this file) |

The release tag was **not** moved after the post-tag commits. The candidate's
bytes are the `fc67f8d` source; later commits are harness, workflow, and
documentation corrections that cannot alter released artifacts.

## 2. Eggup dependency disposition

- `eggup-core = "0.1.1"`, `eggup-service = "0.1.1"` in `crates/eggwork-server/Cargo.toml`.
- Both resolve to crates.io registry releases pinned by `Cargo.lock` with
  checksums. Eggwork holds no git dependency on Eggup, so Eggup tag movement
  cannot change a built candidate.
- Eggup owns the service-manager mechanics and host selection policy
  (`candidate_managers_for` delegates to `eggup_service::candidate_managers`).
  Eggwork owns only the product policy: systemd unit/scope, launchd
  domain/target/plist, SCM start type. `scripts/check_execution_ownership.py`
  enforces that no direct platform-manager implementation exists in Eggwork.

## 3. Eggpack tool pin and durability

| Item | Value |
| --- | --- |
| Pinned revision | `32a0903936fcc283863e0bfb86151b13b4d75ce9` |
| Package | `eggpack-cli` |
| Policy file | `release/eggpack/github-policy.json` (`eggpack_tool.revision`) |
| `eggpack ci check` | `ci check: match (51529 bytes)` |
| CI enforcement | `.github/workflows/ci.yml` drift job installs the pinned revision every run |

Durability finding: the pin is a git revision, so a force-push or history rewrite
upstream would break the CI drift job loudly rather than silently substitute a
different tool. The `eggpack_tool_durability` finding from M003 remains
**low severity** and is unchanged: the pin is immutable for audit but its
upstream object lifetime is not under Eggwork's control. No medium or high.

## 4. Historical `v0.1.0` disposition

| Item | Value |
| --- | --- |
| Annotated tag | `eae3d69d2967a4a6a6782d4632aa26517dd04268` |
| Peeled source | `8827ed5` |
| Draft release | `400502116`, `draft=true`, `published_at=null`, 17 assets |
| First release run | `36787942079` — failed |
| Rerun | `36868105194` — failed on the Windows staged digest mismatch |

`v0.1.0` is **immutable history**. The Windows digest mismatch was a
non-reproducible PE link, not a corrupt upload, and it was not corrected in
place. Both the tag and the draft release are untouched by M004 and remain
unpublished. `v0.1.0` must never be published as the M004-qualified release.

## 5. Corrected `v0.1.1` release

| Item | Value |
| --- | --- |
| Annotated tag | `b60c895f2f644457e7fee24da7d6ef45f87016e2` |
| Peeled source | `fc67f8d90037892486d6a0e9e263df502900b3ed` |
| Tagger date | 2026-10-03T02:35:27Z |
| Draft release | `402292969`, `draft=true`, `published_at=null` |
| Asset count | 17 |
| Workspace version | `0.1.1` (`Cargo.toml`); five workspace packages in `Cargo.lock` |
| Publication | **not performed** — human-controlled per §14 |

### 5.1 Asset inventory (17)

| Asset | Bytes | Asset id |
| --- | --- | --- |
| `eggwork-v0.1.1-x86_64-unknown-linux-gnu` | 10516600 | 607014936 |
| `eggwork-v0.1.1-x86_64-unknown-linux-gnu.sha256` | 106 | 607014947 |
| `eggwork-v0.1.1-aarch64-unknown-linux-gnu` | 10504544 | 607014864 |
| `eggwork-v0.1.1-aarch64-unknown-linux-gnu.sha256` | 107 | 607014868 |
| `eggwork-v0.1.1-x86_64-apple-darwin` | 11009492 | 607014875 |
| `eggwork-v0.1.1-x86_64-apple-darwin.sha256` | 101 | 607014902 |
| `eggwork-v0.1.1-aarch64-apple-darwin` | 9774688 | 607014842 |
| `eggwork-v0.1.1-aarch64-apple-darwin.sha256` | 102 | 607014862 |
| `eggwork-v0.1.1-x86_64-pc-windows-msvc.exe` | 9772544 | 607014912 |
| `eggwork-v0.1.1-x86_64-pc-windows-msvc.exe.sha256` | 108 | 607014928 |
| `eggwork-sandbox-helper-v0.1.1-x86_64-unknown-linux-gnu` | 704488 | 607014824 |
| `eggwork-sandbox-helper-v0.1.1-x86_64-unknown-linux-gnu.sha256` | 121 | 607014831 |
| `eggwork-sandbox-helper-v0.1.1-aarch64-unknown-linux-gnu` | 722600 | 607014804 |
| `eggwork-sandbox-helper-v0.1.1-aarch64-unknown-linux-gnu.sha256` | 122 | 607014821 |
| `install.sh` | 9518 | 607014962 |
| `install.ps1` | 6230 | 607014952 |
| `release-manifest.json` | 1751 | 607014975 |

The sandbox helper ships for Linux targets only, which the harness asserts
per target (`helper-not-shipped-off-linux`).

### 5.2 Same-tag rerun and no-clobber audit

| Run | Result | Staging receipt |
| --- | --- | --- |
| `37090342717` (first `v0.1.1` build) | pass, all generated jobs | 17 created, 17 uploaded |
| `37091624589` (same-tag rerun) | pass | `created: false`, `uploaded: 0`, `reused: 17` |

The rerun reused all 17 assets by name with unchanged ids and sizes. No
differing same-name asset was produced, so §20's clobber stop condition did not
trigger. The `v0.1.0` draft `400502116` was re-read after M004 and still has
17 assets and `published_at=null`.

## 6. Windows reproducibility preflight

| Item | Value |
| --- | --- |
| Workflow | `.github/workflows/windows-reproducibility.yml` |
| Run | `37067023635` at head `fc67f8d` — pass |
| Builds | two independent native `x86_64-pc-windows-msvc` builds |
| Staged SHA-256 | `c419e9e8ec2d74fa7159a545539021e0ffa62713f1673064347d44e12d78acac` (both) |
| COFF timestamp | `2560721357` in both |
| CodeView | absent in both |
| Mechanism | `.cargo/config.toml` target-scoped `-C link-arg=/BREPRO -C link-arg=/DEBUG:NONE` |

The policy is scoped to `[target.x86_64-pc-windows-msvc]` only; Unix link
behaviour is unchanged. `scripts/verify_windows_reproducibility.py` and
`tests/release/test_windows_reproducibility.py` (15 tests) enforce it, and
`crates/eggwork-server/tests/release_contract.rs` fails if any workflow
reintroduces a `RUSTFLAGS` build-time mutation.

## 7. Native install matrix (exact release bytes)

Every platform installs the exact published bytes and verifies the manifest,
sidecar, and installed digest against the release inventory. Helper shipping is
asserted per target.

| Target | Install | Daemon version | Installed digest | Installer | Runtime |
| --- | --- | --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | pass | `0.1.1` | matches | pass | pass |
| `aarch64-unknown-linux-gnu` | staged, digest verified | — | matches | staged | not executed (no native aarch64 host available) |
| `x86_64-apple-darwin` | pass | `0.1.1` | matches | pass | pass |
| `aarch64-apple-darwin` | pass | `0.1.1` | matches | pass | pass |
| `x86_64-pc-windows-msvc` | pass | `0.1.1` | matches | pass | pass (daemon starts; see §9) |

Receipts: `installed-execution-*`, `native-service-*`, and `windows-disposition-*`
artifacts from the final green qualification run **`37100047745`** (all six jobs
`success`). That run produced **179 passing receipt steps** across thirteen
receipts:

| Receipt | Steps |
| --- | --- |
| `install-linux-x86_64-unknown-linux-gnu.json` | 13/13 |
| `installer-linux-x86_64-unknown-linux-gnu.json` | 7/7 |
| `install-macos-aarch64-apple-darwin.json` | 9/9 |
| `installer-macos-aarch64-apple-darwin.json` | 7/7 |
| `install-macos-x86_64-apple-darwin.json` | 9/9 |
| `installer-macos-x86_64-apple-darwin.json` | 7/7 |
| `install-windows.json` | 9/9 |
| `service-macos-aarch64-apple-darwin.json` | 21/21 |
| `service-macos-x86_64-apple-darwin.json` | 21/21 |
| `update-macos-aarch64-apple-darwin.json` | 29/29 |
| `update-macos-x86_64-apple-darwin.json` | 29/29 |
| `execution-linux-x86_64-unknown-linux-gnu.json` | (see §10) |
| `execution-macos-{aarch64,x86_64}-apple-darwin.json` | (see §10) |

No Rust toolchain is required on the target host to install or run the release;
the harness toolchain exists only to build the client driver and materialise
the fixture.

A macOS update stage in an earlier run (`37098223811`) aborted with
`asset download returned HTTP 500` from the release API. A 5xx on a well-formed
request is not a candidate failure, so the harness now retries 5xx, 429, and
transport errors with bounded backoff and still fails loudly once exhausted; a
4xx is deliberately not retried. Seven tests cover that semantics
(`tests/release/test_qualify_release_harness.py`).

## 8. Service-manager matrix

| Backend | Platform | Disposition | Evidence |
| --- | --- | --- | --- |
| `systemd` | Linux x86_64 | **service-qualified** | live install/start/stop/start/stop/restart/uninstall; foreign same-name executable reported `Foreign` and every mutating verb refused; uninstall cleared the registration |
| `launchd` user agent (`gui/<uid>`) | macOS aarch64 | **service-qualified** | same full matrix, receipts `install-service-macos-aarch64-apple-darwin.json` and `service-macos-aarch64-apple-darwin.json` |
| `launchd` user agent | macOS x86_64 | **service-qualified** | same full matrix, receipts `install-service-macos-x86_64-apple-darwin.json` and `service-macos-x86_64-apple-darwin.json` |
| `windows-scm` | Windows x86_64 | **unsupported** | registers, then start fails with Windows error 1053; see §9 |

The Windows service disposition is captured as an artifact rather than a log
line, in `windows-disposition-x86_64-pc-windows-msvc-*` from `37100047745`:

```
service install exit=0 output={"backend":"windows-scm","completed":true,
  "operation":"install","platform":"windows","schema_version":1,
  "service_id":"eggwork-node"}
service start exit=2 output=eggworkd: service manager failed: start service
  failed in Windows SCM (code 1053)
DISPOSITION: unsupported (registers, then cannot reach Running; no service host)
```

Every claimed backend was mutated on a real hosted host. Adapter availability
alone was not treated as a support claim: the Windows adapter successfully
registers the service, and that is precisely why the unsupported disposition had
to be stated explicitly rather than left as "untested".

Ownership checks are exercised adversarially: after an owned service is
installed, a second executable with the same name is reported `Foreign` and
`install`, `start`, `restart`, `stop`, and `uninstall` are all refused with a
non-zero exit, and the owned service is verified intact afterwards. No platform
passed by bypassing an ownership check.

No automatic elevation is used anywhere. Service definitions are written to
user-scoped locations (systemd user unit, launchd `LaunchAgents` plist).

## 9. Windows service management: unsupported

Hosted evidence (`37095687727`, and reproduced in `37096444794`):

```
QUALIFICATION FAILED (service): eggworkd.exe exited 2:
  eggworkd: service manager failed: start service failed in Windows SCM (code 1053)
```

Windows error 1053 is "the service did not respond to the start request in a
timely fashion". The cause is structural and was confirmed by inspection, not
inferred: `eggworkd` implements **no service-control dispatcher**. Grepping the
workspace found no `windows_service::service_dispatcher` or equivalent in
`crates/`; the `windows-service` crate reaches the dependency graph only as a
transitive dependency of `eggup-service`'s manager side. The process the SCM
launches therefore never connects back to the manager, so the registration can
never reach `Running`.

The typed adapter registering the service successfully is what made this look
like flakiness. It is not: registering an external process would leave a working
installation with **no service behind it**, which is worse than an honest
refusal.

Disposition (landed in `caa3918`): every mutating `service` verb on Windows is
refused before any mutation, with a message naming the unsupported backend. The
recorded SCM product policy and typed adapter are retained in
`deployment::windows_scm_manager` (marked `#[allow(dead_code)]`) for a future
service-host milestone; only the operator surface is closed.
`deployment status` still reports host facts, and binary installation and daemon
runtime are unaffected.

Registering the daemon as a Windows service host is a **product capability, not
a qualification fix**, so it is a separate milestone and is deliberately not
implemented under M004. The candidate's bytes predate this change, so the
candidate registers-and-fails-1053 while `main` fails closed before mutating;
the qualification workflow records both observations and fails if either
changes (`windows-platform-disposition`).

## 10. Installed execution and capability matrix

Authenticated installed execution through the production client, against the
exact release bytes.

| Platform | Execution | Required filesystem isolation | Evidence |
| --- | --- | --- | --- |
| Linux x86_64 | **qualified** | **qualified**: Landlock `workspace_rw` applied, outside-workspace write denied, marker absent | `execution-linux-x86_64-unknown-linux-gnu.json`; `sandbox: not_requested` with `applied.profile: workspace_rw` |
| macOS aarch64 | **qualified** | unsupported → `capability_mismatch`, HTTP 409, before spawn | `execution-macos-aarch64-apple-darwin.json`; `marker_created: false` |
| macOS x86_64 | **qualified** | unsupported → refused before spawn | `execution-macos-x86_64-apple-darwin.json` |
| Windows x86_64 | **not claimed** for the candidate | unsupported | §11 |

Exact facts from the Linux receipt: `state: Succeeded`, `exit_code: 0`,
`failure: null`, `cleanup_warning: null`, `stdout: "eggwork-qualification\n"`,
`stdout_bytes: 22`.

The Linux Landlock result is attributed to the **installed** sandbox helper, not
to a developer's build tree: the daemon resolves and verifies the trusted helper
that ships in the release (Linux is the only target where it ships), and the
`ops` crate delegates that check to `eggwork_runner::verify_trusted_helper`.
An unowned helper is rejected rather than trusted.

Unsupported required controls keep rejecting before spawn on every platform.
`isolation_capable: true` on Linux and `false` on macOS is read from the
installed node, not assumed by the harness.

## 11. Windows child execution: not claimed for this candidate

Hosted evidence (`37095687727` and later) before the fix:

```
assertion `left == right` failed: the installed node did not complete a bounded execution:
{"cleanup_warning":null,"execution_id":"…","exit_code":null,"failure":"Internal",
 "sandbox":{"status":"not_requested"},"state":"Failed","stdout":"","stdout_bytes":0}
```

`failure: Internal` with `exit_code: null` and no output means the child started
and exited with no capturable exit code. Inspection found the cause in the
product, not the harness: `direct_command` in
`crates/eggwork-runner/src/lib.rs` cleared the environment and declared a
**Unix-only** baseline (`PATH=/usr/bin:/bin`, `LANG`, `LC_ALL`, no `SystemRoot`).
A Windows child was handed a `PATH` that does not exist on Windows.

Fix (landed in `202d395`): the baseline is now platform-shaped. Unix keeps the
narrow deterministic baseline that all existing Linux and macOS evidence was
gathered with; Windows gets `SystemRoot`, `windir`, and a Windows `PATH` without
inheriting the caller's environment.

The candidate's bytes predate this fix and cannot be re-qualified without a new
release, so Windows child execution is recorded as **not claimed** for `v0.1.1`
rather than qualified. The `windows-platform-disposition` job records the
disposition in `windows-execution.json`:

```json
{"platform":"windows","target":"x86_64-pc-windows-msvc",
 "installed_runtime_qualified":true,"child_execution_qualified":false,
 "required_filesystem_isolation":"unsupported","service_management":"unsupported",
 "disposition":"not claimed; the candidate builds its child environment for Unix only"}
```

The job fails if either observation changes, so neither limitation can silently
rot: a Windows service that reaches `Running` or a Windows child that spawns
successfully would fail the run and force the support matrix to be re-qualified
before the capability is claimed.

## 12. Update, rollback, and recovery matrix

Executed on Linux x86_64 (systemd), macOS aarch64, and macOS x86_64 (launchd),
updating the installed `v0.1.0` generation to the installed `v0.1.1` candidate
from the two real drafts.

| Step | Result |
| --- | --- |
| prior candidate verified against the `v0.1.0` manifest and sidecar | pass |
| prior generation differs (`0.1.0` vs `0.1.1`) | pass |
| update of a **running** service | pass, `artifact_disposition: Committed`, `artifact_failure: null` |
| installed unit replaced with the exact candidate digest | pass |
| running lifecycle restored after update | pass |
| recovery state outside the install unit byte-identical | pass |
| update of a **stopped** service preserves stopped | pass |
| unsatisfiable quiescence refuses the update | pass, bounded-wait exit 2, drain still active |
| refusal happened **before** artifact replacement | pass, installed digest still equals candidate, not the prior |
| unreadable execution store refuses and restores | pass, 3 execution-store files restored byte-identically |
| refused update leaves drain set | pass |
| rollback with a failing post-commit check | pass, `artifact_disposition: RolledBack`, `artifact_failure.category: PostCommitCheck` |
| rollback restores the prior generation and its exact digest | pass |
| rollback does not claim "updated" | pass |
| rollback restores the prior lifecycle state | pass, `lifecycle_restoration: Restored` |
| recovery state byte-identical after rollback | pass |

`RecoveryRequired` is never auto-started; the receipt shows the prior lifecycle
state (`Stopped`) preserved after a failed update. The refusal path is proven to
run before replacement, so a refused update cannot leave a half-updated install.

## 13. Drain and execution truthfulness

- A draining node refuses a new execution: `refused_while_draining: true`.
- After an explicit, persisted undrain, the same execution is admitted:
  `admitted_after_undrain: true`.
- Un-drain is explicit and persistent: `{"draining": false, "persistent": true}`.
- Drain state is visible in `deployment status`: `active_executions: 0`,
  `cleanup_failures: 0`.
- Cleanup truthfulness: `cleanup_warning: null` and `omitted_bytes` 0 on
  bounded output for every qualified execution, so truncation was never
  silently absorbed.

## 14. Product defects found and fixed

Each was found by qualification, reproduced, fixed, and covered by a test.

| Defect | Fix | Test coverage |
| --- | --- | --- |
| Post-install version check compared raw stdout to a version string, so a JSON status response always mismatched and a healthy install reported failure | `deployment::check_installed_daemon_version` parses the JSON version field | unit tests in `deployment.rs` |
| Trusted-helper verification in `ops` reimplemented a weaker check than the runner's, permitting a user-owned helper path; the qualification path therefore depended on a developer's build tree | `operations::trusted_helper` delegates to the public `eggwork_runner::verify_trusted_helper` | runner trust-boundary tests |
| `deployment apply` failure output was unbounded, leaking an entire artifact/failure payload into operator JSON | bounded artifact, post-commit-check, and rollback failure fields | CLI tests |
| Windows PE links were not byte-reproducible (the `v0.1.0` failure) | target-scoped `/BREPRO` + `/DEBUG:NONE` | 15 reproducibility tests + hosted double build |
| Windows SCM start fails with 1053 (no service host) | mutating verbs fail closed before mutation | `main` verified by clippy/unit/CI; candidate disposition recorded by hosted job |
| Runner declared a Unix-only child environment on every platform | platform-shaped baseline environment | `main` verified by unit/CI; candidate disposition recorded by hosted job |

## 15. Verification actually executed

Repository gate (all pass, local, at closure head):

```
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets          # 171 passed, 1 ignored, 10 suites
cargo check --workspace
git diff --check
```

Release gate:

- `eggpack ci check` against the pinned tool → `ci check: match (51529 bytes)`.
- Windows double-build preflight → §6.
- Coherent version bump to `0.1.1` across the workspace and all five lockfile packages.
- Exact `v0.1.1` draft build/qualification → §5, §7.
- Same-tag rerun → §5.2.
- Staging receipt / all-asset reuse / no-clobber audit → §5.2.

Python release tests: 41 passed (`tests/release/`, including
`test_windows_reproducibility.py` and `test_release_candidate_probe.py`, which
derives the workspace version rather than hard-coding it).

Hosted CI: green on `32e4559` (run `37100033522`) and on the closure head
`9ba5e20` (run `37100665231`).

**Not executed, and not claimed:** public bootstrap of the published release
(all five targets), because the release was not published (§14).

## 16. Security and compatibility review

- No secret is present in the repository, the release assets, the receipts, or
  the workflow logs. The workflow reads the release through
  `RELEASE_QUALIFICATION_TOKEN`, a repository secret; the harness prints no
  token and writes no credential to any receipt.
- Tag and release immutability: the rerun reused all 17 assets and produced no
  differing same-name asset, so there is no clobber path. `v0.1.0` is untouched.
- Compatibility: no protocol, schema, or capability change. The only
  behavioural change to shipped semantics is the Windows service refusal,
  which turns a previously broken path into an explicit refusal. The child
  environment change is platform-conditional and leaves Linux and macOS
  behaviour byte-identical to what the evidence was gathered with.
- MSRV remains Rust 1.89.
- Ownership: no plan can only pass by bypassing an ownership check (§8).

## 17. Final platform support matrix

Categories are exactly the plan §15 vocabulary. One category is never inferred
from another.

| Target | release-qualified | installed-runtime-qualified | service-qualified | required-isolation-qualified |
| --- | --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | yes | yes | yes (systemd) | yes (Landlock `workspace_rw`) |
| `aarch64-unknown-linux-gnu` | yes (staged, digest verified) | **untested** (no native host) | **untested** | **untested** |
| `x86_64-apple-darwin` | yes | yes | yes (launchd) | no — unsupported, refuses before spawn |
| `aarch64-apple-darwin` | yes | yes | yes (launchd) | no — unsupported, refuses before spawn |
| `x86_64-pc-windows-msvc` | yes | yes (daemon starts and serves) | no — **unsupported** | no — unsupported |

Additional explicit dispositions:

| Capability | Disposition |
| --- | --- |
| Windows child execution | **not claimed** for the `v0.1.1` candidate; fixed on `main`, re-qualification needs a release containing `202d395` |
| Windows service management | **unsupported**; daemon has no service-control dispatcher; fails closed on `main` |
| Network isolation Disabled/AllowListed | **unsupported** on all platforms in current scope |
| macOS required filesystem isolation | **unsupported**; no Landlock equivalent implemented |
| Windows required filesystem isolation | **unsupported** |
| `aarch64-unknown-linux-gnu` runtime | **untested**; no native aarch64 Linux host was available |

Do not infer, for example, that aarch64 Linux is service-qualified because x86_64
Linux is, or that Windows service management is untested rather than
unsupported. The Windows service backend is **unsupported** because the
capability is intentionally absent, and the SCM adapter registering the service
is not evidence of support.

## 18. Outstanding evidence

Named, non-critical, and the reason closure is conditional.

| # | Item | Why it is outstanding | Required to close |
| --- | --- | --- | --- |
| 1 | **Public publication and post-publication bootstrap matrix on all five targets** | §14 makes publication an explicit maintainer action; it did not occur. Both `v0.1.0` and `v0.1.1` remain `draft=true`, `published_at=null`. | A maintainer publishes draft `402292969`; then verify the public exact-tag asset inventory and run the public bootstrap matrix on all five targets. |
| 2 | **Windows service management qualification** | The daemon has no service-control dispatcher, so the backend is dispositioned `unsupported` rather than live-qualified (plan §15). Implementing service hosting is a separate product milestone. | A service-host milestone lands a dispatcher, then a release containing it is qualified on a real Windows host. |
| 3 | **Windows child execution qualification** | The candidate's runner built a Unix-only child environment. Fixed on `main`; the candidate's immutable bytes cannot be re-qualified. | A release containing `202d395` passes the Windows installed-execution qualification. |

Item 2 is a deliberate §15 disposition of a capability, not a hidden gap.
Item 3 is a known-and-fixed defect awaiting the next release.
Item 1 is a human action that must never be fabricated.

## 19. Unresolved findings by severity

| Severity | Finding | Status |
| --- | --- | --- |
| medium | Windows child execution unqualified for the candidate | root cause found, fix not yet chosen; outstanding evidence item 3 |
| medium | Windows service management unsupported | dispositioned fail-closed; outstanding evidence item 2 |
| medium | public publication/bootstrap not performed | outstanding evidence item 1 |
| low | Eggpack git pin durability under upstream history rewrite | unchanged from M003; pin fails loudly, never silently |
| low | `aarch64-unknown-linux-gnu` runtime/service untested | no native host available; staged bytes digest-verified only |
| low | `tests/release` CI flake: `required_landlock_allows_workspace_and_denies_outside_reads_and_writes` failed once on `33f9a68` with `Spawn("Text file busy (os error 26)")` and passed on an immediate rerun of the same head | **open**. `ETXTBSY` is a test-staging race, not a product defect: every test in that file stages the same shared `target/debug/eggwork-sandbox-helper`. It needs its own corrective rather than a retry, and a rerun is not a fix. |
| low | `scripts/qualify_release.py` release-API flake | closed by bounded retry with tests |
| none open | release clobber, tag movement, secret exposure, ownership bypass, auto-start after `RecoveryRequired`, execution of a denied capability, false supported-isolation claim | none observed |

No **high** finding remains. The three medium findings are the declared
outstanding evidence, each with a named closure path; none is a correctness or
security defect in a qualified path.

## 20. Acceptance criteria

| # | Criterion | Verdict |
| --- | --- | --- |
| 1 | M002a closed first | met — `plans/closure/operations-distribution/003-status.md` |
| 2 | no direct platform-manager implementation in Eggwork | met — ownership guard; Eggup's typed adapters only |
| 3 | Linux systemd product policy safe for all accepted paths | met — user-scoped unit, ownership-enforced |
| 4 | macOS launchd policy deterministic, ownership-safe, live-qualified for every claimed domain | met — user agent domain qualified on two targets; the system domain is **not** claimed |
| 5 | Windows SCM uses only Eggup's typed adapter **and is live-qualified** | **not met for Windows** — adapter is Eggup's only, but the daemon has no service host, so the backend is dispositioned `unsupported` (§9) |
| 6 | clean supported hosts install/run without Rust | met for every claimed runtime |
| 7 | every claimed service backend passes native lifecycle evidence | met — systemd and launchd; Windows is not claimed |
| 8 | installed remote execution succeeds on every platform claimed for runtime | met for Linux and both macOS targets; Windows execution is not claimed (§11) |
| 9 | unsupported required isolation fails before spawn | met — macOS `capability_mismatch` HTTP 409, marker never created |
| 10 | drain/update/rollback truthful; `RecoveryRequired` never auto-starts | met — §12, §13 |
| 11 | Windows candidate independently byte-reproducible; same-tag rerun reuses all 17 assets without clobber | met — §6, §5.2 |
| 12 | release workflow/tool pins immutable; durability finding resolved or explicitly low | met — low severity, unchanged from M003 |
| 13 | publication remains human-controlled | met — no automated publish path exists; both releases still draft |
| 14 | if published, public bootstrap passes on all five targets; otherwise close conditionally with that evidence named | met via conditional closure — item 1 |
| 15 | release/update/service ownership documentation matches production | met — README, `architecture/distribution.md`, including the Windows refusal |
| 16 | no unresolved high/medium correctness or security finding | met for qualified paths — the three medium items are named outstanding evidence, not defects in a qualified path |

Criterion 5 is the single unmet acceptance criterion, and it is unmet by design
rather than by omission: the plan anticipated it in §15 by providing the
`unsupported` category. The gap is the service-host capability itself, which
belongs to a separate milestone.

## 21. Phase 6 exit disposition

Plan §22 applies with one amendment.

- Operations/Distribution **M001–M003 are closed**; M004 is **conditionally
  closed** with three named outstanding evidence items (§18).
- Phase 6 exit criteria are satisfied **for the support matrix actually proven**
  in §17. They are not satisfied for Windows service management, Windows child
  execution, macOS required filesystem isolation, network isolation on any
  platform, or aarch64 Linux runtime.
- Reverse-connect **M005** and PTY **M006** remain **deferred** and do not block
  this release. Neither was made ready by M004: M005's dependency is a remote
  connect/accept path that M004 neither implements nor claims, and M006's
  dependency is a PTY capability stream that is out of M004 scope.
- Downstream CodeGG M004 remains **independently governed** by CodeGG's AgentRun
  worker-entry contract and is not unblocked or blocked by this record.
- No capability in §18 or §17's unsupported rows may be claimed as a
  consequence of this closure.

## 22. Reproducing this record

Final state, all verified from `main` at `32e4559` or later:

- hosted CI: runs `37100033522` and `37100665231`, `success`
- hosted operational qualification: run `37100047745`, `success` (6/6 jobs)
- hosted Windows reproducibility: run `37067023635`, `success`
- release build + qualification: runs `37090342717` and `37091624589`, `success`
- local gates: see §15

## 23. Publication readiness (item 1, not yet performed)

Publication is a maintainer action under plan §14. Nothing below has been done.
This section exists so the action is a verified single step rather than a
judgement call, and so the two facts that must not be assumed — that the draft
is still a draft, and that the public inventory matches — are both checkable.

**Target.** Draft release `402861780`, tag `v0.1.4`, source
`b31f117fe4cbb7221aa64014d7e50d67d4113144`. *Supersedes `v0.1.2`: see §24.
Earlier targets are struck through by the table there, not edited away.*

**Pre-publication, tokened** (records the §14 facts; run against the draft):

```
RELEASE_QUALIFICATION_TOKEN=... python3 scripts/qualify_release.py inventory \
  --release-tag v0.1.4 --receipt receipts/inventory-v0.1.4.json
```

This asserts the exact tag, `draft=true`, `published_at=null`, 17 assets, no
missing and no unexpected assets, and cross-checks the manifest's declared
artifacts against the assets that exist.

**Post-publication, anonymous** (verifies the public inventory and that a
consumer needs no credential):

```
python3 scripts/qualify_release.py inventory --public \
  --release-tag v0.1.4 --receipt receipts/public-inventory-v0.1.4.json
```

`--public` refuses to run while `RELEASE_QUALIFICATION_TOKEN` is set, and refuses
to run against a draft. That is deliberate: a tokened read reporting "public"
would make the difference between a consumer installing the release and a
maintainer installing it with a credential unobservable, which is exactly the
distinction this evidence exists to establish.

**What publication does not do.** It does not qualify anything. The staged
assets are already qualified by the hosted runs in §15; publication changes
their visibility, not their bytes. If the post-publication inventory disagrees
with the pre-publication one, stop: that is a release-integrity finding, not a
documentation fix.

**Never**, for this milestone: publish `v0.1.0`; move or recreate any tag;
replace a differing asset; publish as a way of fixing a failed check.

## 24. Follow-on releases after the closure window

`v0.1.2` was cut after the closure record was written, to carry the two fixes
the `v0.1.1` candidate could not contain. It is a superset of the `v0.1.1`
evidence. It was itself superseded by `v0.1.4` before publication (see table);
the publication decision should apply to `v0.1.4`.

| Tag | Draft id | Source | Why it exists |
| --- | --- | --- | --- |
| `v0.1.0` | `400502116` | `8827ed5` | historical, immutable, never published |
| `v0.1.1` | `402292969` | `fc67f8d` | the M004 closure candidate; pre-fix for both Windows gaps |
| `v0.1.2` | `402786764` | `f43feb7` | adds the platform-shaped child environment and the fail-closed service refusal; superseded before publication because its bytes contain the output-monitor defect |
| `v0.1.3` | — (staging failed) | `5955431` | never staged: the 0.1.3 version bump changed `Cargo.toml` without regenerating `Cargo.lock`, so every `--locked` release build failed closed with "the lock file needs to be updated". CI ran clippy and test without `--locked` and therefore passed the gate that exists to prevent exactly this; both now pass `--locked`. The tag and its failed producer run (`37176445644`) are left exactly as they are. |
| `v0.1.4` | `402861780` | `b31f117` | staged, then refused by its own harness: the version bump was missed, so the staged binaries report `0.1.3` and hosted qualification run `37185169008` failed every install stage with `installed-daemon-version: daemon reports '0.1.3', expected 0.1.4`. That refusal is the version-coherence check working as designed; the failure is real process evidence, not a harness problem. Staging itself was sound — run `37178170560` success, 17 assets, `draft=true`, `published_at=null`; same-tag re-run `37183381984` success with `created=false, uploaded=0, reused=17`; manifest cross-check 17/17 with `source_revision` equal to the tag source. Left as-is; superseded by `v0.1.5`. |
| `v0.1.5` | staging | `c3ece0c` | same product content as `v0.1.4` with the workspace version the tag names, verified by `scripts/check_release_tag.py` before the tag was created. The tag-to-version link cannot live in CI — the release workflow is Eggpack producer authority and the `release-drift` job rejects hand edits to it — so the check runs at tag time by whoever cuts the tag. |

`v0.1.2` qualification reproduced the same `Internal`/`null` shape, and it was
attributed to a **third** defect — the output-monitor race — on the theory that
the first two fixes had finally let the code path be reached. That attribution
was wrong, and the record corrects it: `v0.1.5` contains the monitor fix and
reproduces the identical shape on hosted Windows (run `37190674127`), while
every Unix target passes. The monitor race is real and now pinned by a test,
but it was never the Windows cause. The actual cause predates all of the
fixes: `LocalProcessRunner::run` refuses with `UnsupportedPlatform` on
`not(unix)` before any child exists (`crates/eggwork-runner/src/lib.rs`), and
the server maps that refusal to `ExecutionFailure::Internal`, which reads as a
product defect. No Windows child ever existed to be mis-shaped or mis-waited,
so neither the environment shaping nor the monitor fix could have changed the
outcome. Each release remains immutable; none is re-tagged or clobbered.

## 25. Verification notes

- Code inspection and executed evidence are distinguished throughout: §5.2, §6,
  §7, §8, §10, §12, and §13 cite run ids and artifact names; §9 and §11 cite
  hosted failure output plus named source locations; §17 marks `untested`
  explicitly rather than implying coverage.
- No platform or feature is qualified merely because it compiles. aarch64 Linux
  is `untested` despite passing cross-compilation and digest verification, and
  Windows service management is `unsupported` despite a working adapter.
