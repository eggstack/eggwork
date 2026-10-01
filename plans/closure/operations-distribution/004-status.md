# Operations M004 — Operational and Release Qualification

Status: blocked; implementation and partial qualification evidence recorded

Reviewed Eggwork baseline: `d98dcc352ffa0e948bf62150ccb65e7c437e15b0`

Implementation commit: `b760c1a1705bcb2bcb894b78205fbc3befe2211d`

Prerequisite: Operations M002a closed at `c33e9d6fded7a80a377d6e6748cc9cdb2b02acbc`.

## Release input identity

- Annotated tag: `v0.1.0` peels to `8827ed4812bca3c5a749182e09208e3695549f54`.
- GitHub release id: `400502116`; draft, unpublished.
- Current asset inventory after qualification attempt: 17 assets.
- No tag or release asset was replaced. Publication was not attempted.
- Existing draft Linux x86-64 daemon and helper matched both their release
  sidecars and `release-manifest.json`, including the declared size and SHA-256.

## Requirement-to-evidence matrix

| Requirement | Evidence | Result |
|---|---|---|
| M002a recovery-safe lifecycle before update qualification | `plans/closure/operations-distribution/002a-status.md`; Eggup 0.1.1 lifecycle composition and preserved Core receipt | Pass |
| Exact release bytes/version on Linux x86-64 | Authenticated retrieval of daemon/helper from draft `400502116`; manifest size/digest and sidecar digest checked; both binaries report `0.1.0` | Pass |
| Installed daemon configuration/doctor | Exact daemon copied into a temporary installation tree; `config validate` and `doctor` passed with a no-helper config. A helper-configured `doctor` did not pass this host's sandbox-resource check, so required-Landlock qualification is not claimed. | Partial |
| Linux systemd lifecycle | Exact staged daemon installed as an explicit user-scope `eggwork-node.service`; Eggup-backed install, start, restart, stop and uninstall all completed. `systemctl --user show` observed active/running after start and restart, then inactive/dead after stop. Unit file was removed and service is inactive. | Pass for Linux user scope on this host; system scope untested |
| Systemd command-line policy | `render_systemd_unit` now rejects whitespace and metacharacters outside its literal path alphabet; focused tests pass. No privileged system-scope mutation attempted. | Pass for deterministic policy; system scope unqualified |
| Launchd policy and adapter | XML-escaped argv plist renderer, explicit domain/target manager constructor and CLI dispatch landed; numeric `gui/<uid>` target validation and renderer tests pass. | Implemented; native launchd mutation untested |
| Windows SCM policy and adapter | Typed Eggup SCM descriptor and finite start-type CLI dispatch landed. | Implemented; native SCM mutation untested |
| Manager-specific machine-readable status | Status uses the selected Eggup manager and reports ownership/state; mutating JSON includes backend, operation and completion. | Code present; only Linux service behavior exercised |
| Local-candidate deployment apply | Added local-path-only daemon/helper apply, exact prior digest verification for replacement, persistent drain, quiescence, budgeted installed-version/helper checks, and structured lifecycle receipt output. | Implemented; end-to-end apply/update not exercised |
| Same-tag release rerun / no-clobber | Hosted rerun `36868105194`: all five targets built and qualified, all non-Windows artifacts matched. Stage failed closed on Windows same-name digest mismatch; existing assets were not overwritten. | Blocked |
| Public first-install scripts and bootstrap | Draft remains unpublished; authenticated asset copy was exercised, generated public-origin installers were not run. | Outstanding; requires publication authorization |
| Installed Linux x86-64 mTLS execution and capability/drain controls | Exact staged v0.1.0 daemon accepted a real client mTLS request and completed bounded output with state `Succeeded`; the staged binary recorded `cleanup_warning: "output monitor closed"`. Required isolation rejected with HTTP 409 `capability_mismatch` and did not create an execution; persistent drain rejected a new execution with HTTP 503 until explicitly cleared. A focused runner fix now keeps the overflow watch channel alive until child completion; current-source daemon + normal `NodeClient` smoke completed with `cleanup_warning: null`. | Partial: protocol path proven; exact staged release has the runner warning, and required-Landlock not qualified |
| Update/rollback/recovery matrix and state preservation | No live installed replacement/rollback matrix was run. M002a and Eggup contract evidence is not a substitute for M004's installed-binary evidence. | Outstanding |
| Platform support matrix | Linux x86-64 user-systemd lifecycle: live-qualified. Linux required Landlock: unqualified on this host. Linux system-scope service: untested. macOS launchd and Windows SCM: implemented but untested. Network Disabled/AllowListed: unsupported. | Partial; no cross-platform support claim |
| Publication boundary | Release remains draft; no publication or tag/asset mutation was performed. | Pass |

## Windows rerun mismatch analysis

The rerun's `eggwork-v0.1.0-x86_64-pc-windows-msvc.exe` SHA-256 is
`f3a0a3021bf618431b6d137d3b951c05e2f6f2c5250ccc0dc96207057229f021`; its
rerun manifest and sidecar agree. The staged executable remains
`f5a763cd2f298ab0d668817efec74603ac2a37df3379a079e7fdf9f9b475f190`. PE
inspection found a different COFF timestamp and PDB RSDS signature (`19ab1c9c…`
staged versus `cd86a174…` rerun); section sizes and image size are unchanged.
The mismatch is consistent with the known eggsact M005a Windows byte
reproducibility blocker recorded by current Eggpack planning. Eggpack's staging
job stopped before replacing the asset. Do not rerun staging or reconcile by
clobbering until the upstream blocker has a durable resolution.

## Eggpack pin durability

At M004 start, Eggpack main had advanced to
`32a0903936fcc283863e0bfb86151b13b4d75ce9`, whose registry closes M003h and
records the qualified M003e/f/g behavior on main. Eggwork now pins that exact
immutable revision in `release/eggpack/github-policy.json`; the generated
`.github/workflows/release.yml` was regenerated and `eggpack ci check` reports
`match (51529 bytes)`. The existing `v0.1.0` tag remains the historical source
and was not moved to consume the new pin.

## Verification actually executed

- `python3 scripts/check_execution_ownership.py` — passed.
- `python3 scripts/check_execution_ownership.py --prove-negative-exit` — passed.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — 167 passed, 1 ignored locally,
  including the runner output-reader completion regression.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
  passed after the runner correction.
- `cargo check --workspace` — passed.
- `eggpack ci check --workflow-shape ... --contract ... --github-policy ... --workflow .github/workflows/release.yml` — passed with zero drift.
- Native Linux user systemd lifecycle — passed as detailed above.
- Cross-target macOS check — not a qualification attempt; failed because the
  Linux C compiler does not accept Darwin `-arch` and
  `-mmacosx-version-min` options.
- Cross-target Windows MSVC check — not a qualification attempt; failed because
  the host lacks the MSVC compiler/linker and Windows-compatible native build
  dependencies.
- Exact staged Linux x86-64 installed runtime — the authenticated v0.1.0 daemon/helper from release `400502116` were copied into a temporary installation tree. Their SHA-256 digests were `e2e7a230ef3b8ff5a9d0b91eeee36e65e3e4252cf05784a2cbfcf0ad6d4be16e` and `1a8f1711f18bba53751da5d30dc5471374b59fa8234b25214f8c9a5435f237aa`; both reported version `0.1.0`. The installed daemon passed `config validate` and `doctor` with no helper configured. Authenticated loopback capabilities and execute requests returned the expected protocol features, bounded output, and `Succeeded`; the successful exact-release execution exposed `cleanup_warning: "output monitor closed"`.
- Installed exact-release negative controls — required isolation returned HTTP 409 `capability_mismatch` (counter recorded, no execution accepted); persistent drain returned HTTP 503 `draining` until the explicit `undrain` command. Required Landlock remains unqualified because the helper cannot pass this host's installation ownership/runtime probe.
- Runner correction after the exact-release smoke — the overflow watch sender was kept alive while waiting on child completion, preventing normal output-reader completion from being mistaken for an error/early process exit. Regression coverage repeats short-output completion and closes both output pipes while the child continues to a marker write. The focused regression passed. A current-source daemon with the standard `NodeClient` then completed mTLS execution with exact expected output and `cleanup_warning: null`; this current-source debug build is not the staged release artifact.
- Hosted same-tag run `36868105194` — all producer build/qualification and
  required-target gates passed; stage failed closed on the known Windows
  mismatch.
- Hosted CI run `36872337488` for `b760c1a` — both `rust` and `release-drift`
  jobs passed. The preceding CI run had one transient `ETXTBSY` Landlock test
  failure; the complete suite passed on the next run.
- Current source `eggworkd` + standard `NodeClient` loopback mTLS qualification
  — passed exact bounded stdout and terminal-state checks; `executions show`
  reported `Succeeded`, exit code 0, and no cleanup warning after the fix.
- Hosted CI run `36876251604` — its first attempt passed format, clippy, and
  release drift but had one `required_memory_limit_is_enforced_and_classified`
  sandbox-status timeout. The focused test passed locally; rerunning the failed
  hosted job passed the complete Rust test, check, and release-drift jobs.
- Draft `v0.1.0` was rechecked after the new evidence: still draft, exact tag,
  17 assets; neither staging nor publication was attempted again. Eggpack main
  still resolves to the pinned `32a0903936fcc283863e0bfb86151b13b4d75ce9`.

## Unresolved blockers and disposition

- Upstream eggsact M005a Windows byte reproducibility prevents the required
  byte-identical same-tag rerun/reuse receipt.
- Native macOS and Windows service lifecycle and installed runtime evidence
  remain unproven.
- The immutable staged v0.1.0 Linux daemon retains the observed output-monitor
  cleanup warning. The source fix is not present in the `v0.1.0` tag or its
  draft assets and therefore needs a newly authorized release input before an
  exact-release smoke can qualify the corrected behavior.
- Required Linux Landlock isolation and the full update/rollback/recovery and
  state-preservation matrix remain unqualified.
- Public exact-tag bootstrap smoke remains outstanding while the draft is
  unpublished. It may only be run after an explicit maintainer publication
  action; M004 itself did not publish.
- The live exact-release smoke found an output-reader completion race; a source
  correction and regression test are now recorded in the follow-up commit.
  The old release artifact remains affected, so this corrective does not
  qualify the staged release or satisfy Phase 6 exit.

M004 is **not closed** and Phase 6 exit criteria are not satisfied. There are
no newly unblocked Eggwork implementation plans: Operations M005/M006 and
CodeGG M004 remain deferred. Reopen M004 after the upstream reproducibility
blocker and the missing native qualification evidence are available.
