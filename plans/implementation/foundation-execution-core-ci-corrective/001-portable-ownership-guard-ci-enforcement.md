# Foundation Execution Core CI Corrective C001 — Portable Ownership Guard and CI Enforcement

Status: ready for handoff

Source corrective:

- `plans/subsystems/foundation-execution-core-post-closure-ci-corrective-addendum.md`

Predecessor closure:

- `plans/closure/foundation-execution-core/003-status.md`
- implementation `34ef0811ef80acb9fc5584c5a6db643e106f426c`

Plan-authoring Eggwork baseline:

- `e7d9a8e5a9b66f7c68ff4236c9a074ba81e035d7`

## 1. Objective

Turn Foundation M003's source-level execution-ownership guard into an ordinary portable CI invariant without changing its approved ownership model.

## 2. Required production changes

### 2.1 Remove undeclared `rtk` dependency

Change `scripts/check_execution_ownership.py` so its dependency-direction check invokes the standard Cargo binary available from the Rust toolchain:

```text
cargo metadata --no-deps --format-version 1
```

Do not add a fallback to `rtk` and do not shell through a user profile.

If the repository has an existing portable command helper that is already part of CI, it may be used only if doing so is simpler than direct Cargo invocation.

### 2.2 Preserve scanner behavior

The existing ownership allowlist remains:

- `eggwork-runner` — canonical finite-process lifecycle/resource wrappers;
- `eggwork-sandbox-helper` — target/helper launch required for enforced sandbox/resource setup.

Do not broaden the allowlist to make CI pass.

Preserve:

- comment/string stripping;
- imported `Command::new` detection;
- forbidden dependency checks;
- server dependency-direction checks;
- deterministic synthetic forbidden-spawn self-test.

### 2.3 Wire the guard into CI

Add an explicit GitHub Actions step to `.github/workflows/ci.yml` after checkout/toolchain setup and before or alongside broad Rust verification:

```bash
python3 scripts/check_execution_ownership.py
```

Python is only the guard runner; no third-party Python package should be required.

### 2.4 Prove failure behavior

Add a focused test/self-test path proving that the guard exits nonzero when a forbidden production spawn is introduced.

The current in-process synthetic self-test may satisfy this if its failure behavior is itself exercised deterministically. Do not mutate production source during hosted CI merely to test the guard.

## 3. Verification

Required:

```bash
python3 scripts/check_execution_ownership.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo check --workspace
git diff --check
```

Also inspect the workflow YAML and, when available, record the hosted CI run where the guard step executes successfully.

## 4. Acceptance criteria

1. The guard has no undeclared `rtk` dependency.
2. A clean Rust/Python CI environment can execute it.
3. Ordinary CI runs the guard on push/PR.
4. The negative self-test still proves forbidden spawn detection.
5. Approved process owners remain unchanged.
6. Existing dependency-direction enforcement remains intact.
7. No unrelated CI framework/refactor is introduced.

## 5. Stop conditions

Stop if:

- making the guard portable would require broadening the process-owner allowlist;
- standard `cargo metadata` cannot reproduce the dependency evidence;
- CI would need a private/local tool to run the invariant.

## 6. Closure evidence

Create `plans/closure/foundation-execution-core-ci-corrective/001-status.md` with:

- implementation SHA;
- before/after guard command ownership;
- negative self-test evidence;
- workflow step evidence;
- hosted CI run if available;
- full verification;
- residual findings.
