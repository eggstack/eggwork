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
| Installed mTLS remote execution, capability negative controls, update/rollback/recovery matrix, and state preservation | No live end-to-end installed execution or update/recovery matrix was run. M002a and Eggup contract evidence is not a substitute for M004's installed-binary evidence. | Outstanding |
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
- `cargo test --workspace --all-targets` — 166 passed, 1 ignored locally.
- `cargo check --workspace` — passed.
- `eggpack ci check --workflow-shape ... --contract ... --github-policy ... --workflow .github/workflows/release.yml` — passed with zero drift.
- Native Linux user systemd lifecycle — passed as detailed above.
- Cross-target macOS check — not a qualification attempt; failed because the
  Linux C compiler does not accept Darwin `-arch` and
  `-mmacosx-version-min` options.
- Cross-target Windows MSVC check — not a qualification attempt; failed because
  the host lacks the MSVC compiler/linker and Windows-compatible native build
  dependencies.
- Hosted same-tag run `36868105194` — all producer build/qualification and
  required-target gates passed; stage failed closed on the known Windows
  mismatch.
- Hosted CI run `36872337488` for `b760c1a` — both `rust` and `release-drift`
  jobs passed. The preceding CI run had one transient `ETXTBSY` Landlock test
  failure; the complete suite passed on the next run.

## Unresolved blockers and disposition

- Upstream eggsact M005a Windows byte reproducibility prevents the required
  byte-identical same-tag rerun/reuse receipt.
- Native macOS and Windows service lifecycle and installed runtime evidence
  remain unproven.
- Full installed authenticated execution, required-isolation negative
  controls, and update/rollback/recovery semantics remain unqualified.
- Public exact-tag bootstrap smoke remains outstanding while the draft is
  unpublished. It may only be run after an explicit maintainer publication
  action; M004 itself did not publish.
- No high/medium source-code defect was found in the implemented M004 scope.
  Unresolved items are qualification blockers, not a basis to claim Phase 6
  exit.

M004 is **not closed** and Phase 6 exit criteria are not satisfied. There are
no newly unblocked Eggwork implementation plans: Operations M005/M006 and
CodeGG M004 remain deferred. Reopen M004 after the upstream reproducibility
blocker and the missing native qualification evidence are available.
