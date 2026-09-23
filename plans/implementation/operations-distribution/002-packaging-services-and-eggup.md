# Operations M002 — Packaging, Services, and Eggup Integration

Status: blocked on Eggup platform service-manager adapters

Operations M001 is closed. Current crates.io interfaces provide update transaction
and transport-neutral service contracts, but `eggup-service` 0.1.0 contains no
systemd, launchd, or Windows SCM adapters. Do not duplicate its lifecycle and
ownership machinery in Eggwork to bypass this dependency.

Source roadmap:

- `plans/subsystems/operations-distribution-roadmap.md`

Current Eggup planning baseline is recorded in `plans/registry.md`. Re-check its published crate/API state before implementation.

## 1. Objective

Ship Eggwork as verified prebuilt binaries with explicit service/update ownership while reusing Eggup rather than copying installer/updater/service-management machinery.

## 2. Initial target set

Qualify where CI/toolchains support:

- x86_64-unknown-linux-gnu;
- aarch64-unknown-linux-gnu;
- x86_64-apple-darwin;
- aarch64-apple-darwin;
- x86_64-pc-windows-msvc.

Do not claim runtime qualification from cross-compilation alone.

## 3. Artifact set

Determine whether a release contains:

- eggwork CLI;
- eggworkd;
- runner/sandbox helper binaries if split/required;
- notices/licenses;
- checksum manifest;
- version/provenance metadata.

All components in one installed version must be version-compatible.

## 4. Eggup integration

Prefer Eggup interfaces for:

- acquisition adapter if applicable;
- checksum verification/staging;
- ownership revalidation;
- transactional replacement/rollback;
- service lifecycle hooks;
- install provenance.

Do not duplicate Eggup internals if an interface is nearly available; record the upstream blocker/prerequisite.

Transport/release discovery policy remains Eggwork/adapter-owned according to Eggup's architecture.

## 5. Service integration

Support appropriate:

- systemd;
- launchd;
- Windows SCM;
- non-systemd user lifecycle only where shared Eggup machinery supports it safely.

Service registration must point to the exact owned executable/config and fail closed on foreign/unknown ownership before mutation.

## 6. Upgrade semantics

Document and test:

- stopped node update;
- drained node update;
- running executions during requested update;
- failed candidate validation;
- replacement rollback;
- daemon restart and execution reconciliation;
- helper version skew.

Do not silently stop arbitrary work merely to update. Update policy should normally require/drain or explicit operator force behavior.

## 7. Tests/evidence

- release workflow syntax/permissions;
- asset name/version matrix;
- checksum verification;
- clean install;
- exact binary/version;
- update;
- rollback;
- service start/stop/restart;
- foreign service ownership;
- Linux ARM64 hosted/emulated runtime where truthful;
- macOS/Windows hosted smoke;
- post-update execution recovery.

## 8. Acceptance criteria

1. Clean supported host installs without Rust.
2. Every artifact is checksum/provenance verifiable.
3. Service ownership is explicit.
4. Update uses shared Eggup machinery or records a concrete blocker.
5. Failed update does not leave half-installed component set.
6. Runtime target claims have actual evidence.

## 9. Stop conditions

Stop if Eggup has no stable interface needed for safe integration. Do not fork its transaction/update implementation into Eggwork as a workaround.

## 10. Closure evidence

Create `plans/closure/operations-distribution/002-status.md` with hosted workflow/run links or exact evidence, not only YAML inspection.
