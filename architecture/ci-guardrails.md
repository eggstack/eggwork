# CI and guardrails

Every invariant in [overview.md](overview.md) is stated as a rule that *is enforced*, not merely agreed. This document covers the layer that makes that true: four GitHub Actions workflows, seven Python guard scripts, a workspace lint and toolchain policy, a target-scoped Cargo link policy, a three-module Python test suite, and a pre-submit checklist. None of it is product code, and none of it implements execution, storage, or transport. Its entire job is to convert architectural intent into a red check, and to fail closed when the evidence is ambiguous.

## Responsibility boundary

**This layer owns verification and drift detection.** It owns the questions "does the tree still satisfy the ownership boundary?", "does the checked-in release workflow still match what the pinned producer tool renders?", "does one source revision produce one Windows binary?", "does the installed binary from a staged release actually work on a hosted Linux, macOS, and Windows runner?", and "do the release candidate binaries report the version the workspace claims?". It owns the workspace lint and toolchain policy, the deterministic link policy in `.cargo/config.toml`, and the ordering discipline that turns a version bump into a committed lockfile change.

**This layer does not own product behavior.** No guard script decides whether an execution should be admitted, whether a workspace is safe, or whether a snapshot is durable. Where a guard touches product behavior it does so through the product's own surface — `eggworkd version`, `eggwork-sandbox-helper --version`, `qualify_release.py` driving the installed daemon through the real client. The guards assert; the product decides.

**This layer does not own release publication. A human does.** `CONTRIBUTING.md` states it directly: "Inspect the draft, then publish it. Publication is a human action; the workflow never publishes, creates a tag, or clobbers a differing asset." Every non-staging job in every workflow holds `permissions: contents: read`. The single `contents: write` grant in the repository sits in the `stage` job of the generated workflow, and `release_contract.rs::the_generated_workflow_grants_contents_write_to_only_the_staging_job` asserts that it is the only one.

**This layer does not build the release matrix. Eggpack does.** Eggpack is a separate producer authority. It owns the target set, the artifact forms, the sidecars, the manifest, the installers, the qualification and validation plans, and the staging payload. Eggwork owns the static configuration Eggpack reads; it does not own the workflow that Eggpack renders from it. That split is why `release-drift` installs a pinned Eggpack revision from a checked-in file rather than asserting workflow content itself.

## The invariant-to-enforcement map

| Invariant (from [overview.md](overview.md) §"Invariants that span components" and [execution-ownership.md](execution-ownership.md)) | Enforcing mechanism | What happens on failure |
| --- | --- | --- |
| **1. Process ownership is singular.** Only `eggwork-runner` creates OS children; `eggwork-sandbox-helper` may create the target to apply sandbox mechanics. Server, client, and core must not. | `scripts/check_execution_ownership.py` — `scan_sources()` over `crates/*/src/**/*.rs` against the `APPROVED_PROCESS_OWNERS` allowlist, run as the `rust` job step "execution ownership guard" | Nonzero exit; CI red. Prints `process creation outside approved owners (<matched patterns>)` per offending file. |
| **1b. The ownership guard itself still fails when it should.** | `--prove-negative-exit` step, plus the in-process `self_test()` that runs on *every* invocation | A scanner regression (a dropped spawn pattern) makes the proof step exit 1 with `scanner regression — synthetic forbidden spawn was not detected`. |
| **1c. Crate dependency direction.** `eggwork-server` owns no process machinery and must route through `eggwork-runner`. | `check_execution_ownership.py::check_dependencies()` over `cargo metadata --no-deps --format-version 1` | Nonzero exit naming the offending crate, e.g. `eggwork-core must not depend on eggwork-client`, `eggwork-server enables Tokio process creation; use eggwork-runner`, or `eggwork-server must depend on the canonical eggwork-runner`. |
| **2. No direct service-manager invocation** (`systemctl`, `launchctl`, SCM, `crontab`). | Not machine-checked by a dedicated guard. The one hand-written enforcement is `operational-qualification.yml`'s `windows-installed-execution` job, which fails hard if the Windows service verb *does* mutate. Normatively owned by `execution-ownership.md`. | See [Test coverage and gaps](#test-coverage-and-gaps) — this is the weakest-enforced invariant in the set. |
| **3. Fixed target.** The client names its target explicitly. | Structural: `eggwork-client` is denied a dependency on the node crates by `check_dependencies()`; no workflow resolves a "latest" release. | Nonzero exit from the ownership guard if `eggwork-client` ever gains a `eggwork-runner`/`eggwork-server` edge. |
| **4. Isolation claims are earned, never assumed.** | Exercised, not enforced statically: `installed_release_admits_execution_and_refuses_unsupported_isolation` in `crates/eggwork-server/tests/installed_qualification.rs`, driven from the qualification workflow on three hosted targets. | That ignored integration test fails; the hosted job goes red and its receipts are not uploaded. |
| **5. Fail closed on uncertainty** for guarantees the caller demanded. | Same installed-execution test asserts required isolation is *refused* before the target runs; the Windows job records the service disposition and `throw`s if the refusal ever disappears (`the fail-closed guarantee is gone`). | Hard job failure. The Windows step deliberately ends `exit 0` only *after* recording the refusal, so the daemon's non-zero refusal exit is not inherited as the step's result. |
| **6. Identity-free release configuration.** No tag, SHA, digest, or size under `release/eggpack/`. | `release_contract.rs::static_release_configuration_embeds_no_future_release_identity`, run inside `cargo test --locked --workspace --all-targets`; plus the `CONTRIBUTING.md` prohibition. | Test failure inside the `rust` job. |
| **7. Generated release workflow; no second hand-maintained matrix.** | `release-drift` job running `eggpack ci check` with the pinned revision from `release/eggpack/github-policy.json`; reinforced by `release_contract.rs::the_generated_workflow_is_derived_only_from_static_configuration`, `the_embedded_workflow_shape_matches_the_checked_in_configuration`, and `no_second_hand_maintained_release_matrix_exists`. | `eggpack ci check` exits nonzero on any byte drift; `release-drift` red. |
| **8. No published state.** Config, database, workspace, blob, and artifact state never ship in a release artifact. | `release_contract.rs::no_release_asset_carries_node_state`; `.gitignore` excludes `/target/`, `__pycache__/`, `*.sqlite`, `*.sqlite-shm`, `*.sqlite-wal`, `.env` from the tree. | Test failure; the `*_RUSTFLAGS`/rustflags policy is separately asserted by `the_windows_msvc_build_policy_is_deterministic_and_target_scoped`. |
| **The release workflow never publishes, creates a tag, or clobbers an asset.** | `release_contract.rs::the_generated_workflow_never_publishes_or_mutates_tags`, `the_github_draft_policy_is_draft_only_and_needs_no_wrapper`, `the_generated_workflow_grants_contents_write_to_only_the_staging_job`. | Test failure. Publication remains a human step. |
| **A fixed source revision produces a fixed Windows binary.** | `.cargo/config.toml` `/BREPRO` + `/DEBUG:NONE`, proven by `windows-reproducibility.yml` → `verify_windows_reproducibility.py` (byte-identity + no CodeView record). Static half asserted by `release_contract.rs`. | Preflight exits nonzero with both digests and both COFF timestamps, or `carries a CodeView debug record: RSDS guid=… age=… pdb=… timestamp=…`. Runs *before* any tag exists. |
| **No workflow may replace the target-scoped `rustflags`.** | `release_contract.rs::no_workflow_or_environment_can_replace_the_target_scoped_rustflags`; independently, the pwsh step in `windows-reproducibility.yml` scans `Get-ChildItem Env:` for `RUST.?FLAGS`, and `verify_windows_reproducibility.py::check` re-checks the environment from Python. | Test failure, or `an environment override would replace the target-scoped deterministic rustflags`, or `environment <NAME> replaces the target-scoped deterministic MSVC rustflags; unset it before proving reproducibility`. |
| **The daemon and helper candidates report the workspace version.** | `scripts/validate-daemon-version.py` and `scripts/validate-sandbox-helper.py`, invoked by Eggpack as `python3 <script> <candidate>`; plus `check_release_tag.py` before the tag. | `validation failed: <bounded reason phrase>` on stderr, exit 1. Candidate output is never echoed. |
| **Tag, workspace version, and lockfile agree.** | `scripts/check_release_tag.py <tag>` — run by whoever cuts the tag, because CI cannot know the future tag. | `release tag guard FAILED:` with the mismatches and the remediation line. |
| **The qualification harness itself is correct.** | `tests/release/test_qualify_release_harness.py`, `tests/release/test_release_candidate_probe.py`, `tests/release/test_windows_reproducibility.py` via `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release`. | Unittest failure; the `rust` job red. |

## `scripts/check_execution_ownership.py`

This is the deepest guard in the repository, and the only one that is a real analyzer rather than a set of pattern matches over data files. It has two independent jobs: a **static ownership scan** of production Rust sources, and a **crate dependency-direction assertion** read out of `cargo metadata`. Both must be clean for exit 0.

### What counts as production source

The scanner is deliberately narrow:

```python
for path in sorted((ROOT / "crates").glob("*/src/**/*.rs")):
```

Only `.rs` files under `crates/<crate>/src/`, at any nesting depth, are scanned. `crate_for_source()` maps a path to a crate by taking the second path component (`rel.parts[1]`) and returning `None` for anything else, so the crate identity is positional, not read from a manifest. Consequence: integration tests (`crates/*/tests/*.rs`), `examples/`, `benches/`, and `build.rs` are **not** scanned. That is the intended production boundary — a test that spawns a process is not a product that owns one — but it is a boundary a reviewer should confirm rather than assume.

### The approved-owner allowlist

```python
APPROVED_PROCESS_OWNERS = {
    "eggwork-runner": "canonical finite-process lifecycle and Linux resource wrappers",
    "eggwork-sandbox-helper": "target/helper mechanics for enforced sandbox execution",
}
```

Each entry carries its rationale in the value, and the comment above the dict is explicit about the intent: "These are ownership boundaries, not filename-based exceptions. A new owner requires an architecture review and a named entry here with its reason." A file in an approved crate is *counted*, not rejected: `approved_hits[crate] += len(hits)`, and the passing summary line reports the census — `execution ownership guard passed; approved process owners: eggwork-runner (N spawn sites), eggwork-sandbox-helper (M spawn sites)`. That census is a review signal in its own right; a sudden jump in an approved owner's count is a change in who actually owns processes.

Any hit outside the allowlist is an error, phrased as `process creation outside approved owners (<patterns>)` prefixed with the repo-relative path.

### The spawn patterns

Detection runs on the source with comments and literals blanked out first, so documentation cannot trip it. Four things are then matched:

```python
r"\b(?:std|tokio)\s*::\s*process\s*::\s*Command\b",
r"\bprocess\s*::\s*Command\b",
r"\b(?:spawn_process|spawn_child_process|exec_process)\s*\(",
```

plus an alias-aware check. `process_imports` finds `process::Command` and `process::{...}`; the alias set is seeded with `"Command"` whenever any such import exists, and extended with every `Command as <name>` in a brace group. If any alias is then seen as `<alias>::new(`, the finding `"imported process Command::new"` is added. This is what catches `use std::process::Command; … Command::new("tool")` — the idiomatic spelling that the qualified patterns alone would miss.

`strip_comments_and_literals()` is a hand-written character scanner, not a regex sweep. It blanks `//` to end of line, handles *nested* `/* … */` with a depth counter, handles raw strings with any number of hash delimiters (`r#"…"#`, `r##"…"##`) via a `re.match(r'(?:b)?r(#+)?"', …)` prefix probe, and handles normal/byte strings and character literals with backslash escapes. It preserves newlines and every other byte offset so line-accurate error messages still work. Crucially it disambiguates Rust lifetimes: if a `'` is followed by an alphabetic character, it advances by one and treats the quote as a lifetime, not a char literal.

### The synthetic self-test

```python
def self_test() -> None:
    synthetic = """
        // This comment must not trigger: std::process::Command::new(\"x\");
        let docs = r#\"tokio::process::Command::new(\"x\")\"#;
        use std::process::Command;
        fn forbidden() { let _child = Command::new(\"tool\").spawn(); }
    """
    if not process_spawn_patterns(synthetic):
        raise RuntimeError("negative self-test failed to detect a forbidden spawn")
    safe = '// std::process::Command::new("x");\nlet docs = "Command::new(\\"x\\")";'
    if process_spawn_patterns(safe):
        raise RuntimeError("scanner self-test reported a comment/string as a spawn")
```

`self_test()` runs on *every* invocation, including in `--prove-negative-exit` mode. It asserts two directions: a real spawn is caught, and a comment plus a string that both contain `Command::new(` is not. A guard that stopped stripping comments would start failing every file; a guard that stopped stripping *properly* would start flagging documentation. Both are regressions this catches.

### The dependency-direction assertion

`check_dependencies()` shells out to `cargo metadata --no-deps --format-version 1` with `check=True`, parses the JSON, and indexes packages by name. It then asserts three things:

- For `eggwork-core` and `eggwork-client`, the dependency set must not intersect `{"eggwork-runner", "eggwork-server"}`; for `eggwork-core` it must additionally exclude `eggwork-client`. Error text: `{crate} must not depend on {sorted names}`.
- For `eggwork-server`: if a `tokio` dependency is present, `"process"` must not appear in its `features` list — `eggwork-server enables Tokio process creation; use eggwork-runner`. And no dependency may intersect `FORBIDDEN_PROCESS_DEPS = {command-group, duct, process-wrap, portable-pty, subprocess}` — `eggwork-server adds process-owner dependencies: {names}`. This is the interesting one: it is not just "no new crate", it is a named denylist of the libraries whose entire purpose is process ownership, so re-introducing one is a hard failure rather than a review question.
- `eggwork-server` **must** depend on `eggwork-runner` — `eggwork-server must depend on the canonical eggwork-runner`. The direction is asserted positively, so severing the edge is also a failure. This is why `crates/eggwork-runner/Cargo.toml` is the only crate declaring `tokio = { workspace = true, features = ["net", "process"] }`.

### `--prove-negative-exit`

```python
synthetic_source = (
    "use std::process::Command;\n"
    "fn forbidden() { let _child = Command::new(\"tool\").spawn(); }\n"
)
```

`prove_negative_exit()` writes that source into `tempfile.TemporaryDirectory(prefix="eggwork-ownership-proof-")` — deliberately *outside* `crates/`, so the production scanner does not pick it up — and runs `scan_file_for_spawns()`, the same per-file function used against production sources. If no hits, it prints `execution ownership guard failed: scanner regression — synthetic forbidden spawn was not detected` to stderr and returns 1. If hits, it prints `execution ownership guard negative-exit proof passed: synthetic forbidden spawn detected (<patterns>)` and returns **0**.

Why CI runs it: the guard's value is entirely in its *failure* path, and a failure path is never exercised by a green run. A refactor that drops a pattern, tightens the literal stripper until it blanks real code, or narrows the glob would leave the normal guard step happily green forever. The proof runs on every push so that the guard's own regression surface is continuously tested. Note precisely what it does and does not cover: in this mode `main()` runs `self_test()` and then returns `prove_negative_exit()` — it does **not** run `scan_sources()`, `check_dependencies()`, or the error-aggregation block in `main()`. It proves the *scanner* still detects; it does not prove `main()`'s nonzero-return path end to end.

### Exit codes

`0` on success. `1` for: aggregated source/dependency errors; `self_test()` raising `RuntimeError`; an `OSError`, `subprocess.CalledProcessError`, or `json.JSONDecodeError` anywhere in the run (all caught and reported as `execution ownership guard failed: {error}`); and a failed negative-exit proof. `argparse` supplies its own usage exit for an unknown flag. The success path prints the owner census, then `negative scanner self-test passed; crate dependency direction passed`.

### The rule for allowlisting a new platform backend

`execution-ownership.md` is normative here, and it requires two things *before* the allowlist is touched: the backend must **document its ownership rationale**, and it must **add a negative/positive fixture**. Concretely, for a new platform backend:

1. Write the ownership rationale in `architecture/execution-ownership.md` — why this code must create the target process and why no existing owner can.
2. Add a negative fixture (a spawn that must be detected) and a positive fixture (a spawn that must be tolerated inside the new owner) to `self_test()` or an equivalent unit test, so the scanner behavior is pinned in both directions.
3. Only then add the named entry with its reason to `APPROVED_PROCESS_OWNERS`, in the same change.

The allowlist is keyed by crate name, so a backend that lives inside `eggwork-runner` needs no new entry; a backend that needs its own crate does. Either way, the entry, the doc, and the fixtures land together — the script's own comment ("not filename-based exceptions") is the reminder that the entry is a reviewed architectural decision, not a path exception.

## The `rust` CI job

`.github/workflows/ci.yml`, triggered on every `push` and every `pull_request`, with workflow-level `permissions: contents: read`, on `ubuntu-latest`.

```yaml
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.89.0
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: execution ownership guard
        run: python3 scripts/check_execution_ownership.py
      - name: execution ownership guard negative-exit proof
        run: python3 scripts/check_execution_ownership.py --prove-negative-exit
      - run: cargo fmt --all -- --check
      - run: cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
      - run: cargo test --locked --workspace --all-targets
      - run: cargo check --workspace
      - name: release candidate validator tests
        run: python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
```

Step by step, and what each would miss without it:

1. **checkout** — nothing to miss; it is the substrate.
2. **toolchain `1.89.0` with `rustfmt` and `clippy`** — pins the compiler to the same channel as `rust-toolchain.toml` and installs the two components the next two steps require. Note this action is referenced by *tag* (`dtolnay/rust-toolchain@1.89.0`) here, whereas the generated release workflow pins the same action by *immutable commit SHA*. CI trusting a mutable tag is a deliberate asymmetry and a real drift risk.
3. **`Swatinem/rust-cache@v2`** — build-time only; no guardrail value.
4. **ownership guard** — the singular-owner assertion. Without it, a `Command::new` in `eggwork-server` is a normal-looking refactor that silently forks process ownership away from `eggwork-runner`, taking with it every lifecycle guarantee in [runner-execution.md](runner-execution.md): cleared environment, bounded stdin, concurrent drain, timeout, cancellation, process-group kill, and leader-exit-with-descendants signaling.
5. **negative-exit proof** — as above. Without it, the guard is a check that has only ever returned 0 in practice.
6. **`cargo fmt --all -- --check`** — mechanical formatting. Without it, diffs carry noise and review attention moves from substance to whitespace.
7. **`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`** — `--all-features` matters because the qualification and test features are where the interesting code lives; `-D warnings` is what makes the promotion from "warn" to "fail" real, matching `[workspace.lints.clippy] all = "warn"`. Without it, warnings accumulate and the workspace lint table becomes decorative.
8. **`cargo test --locked --workspace --all-targets`** — the whole Rust suite, including `crates/eggwork-server/tests/release_contract.rs` (36 `#[test]` functions), because `--all-targets` pulls in integration tests. This is where most of the release-configuration invariants in the map above are actually enforced.
9. **`cargo check --workspace`** — a cheap whole-workspace type check *without* `--locked` and without `--all-targets`. It is the fastest possible signal on a broken tree and the first thing a contributor sees go red.
10. **Python release validator tests** — the harness test suite, below.

### Why `--locked` is load-bearing

The workflow comment is the shortest true statement of the failure mode:

> `--locked` is load-bearing. A workspace version bump that does not update `Cargo.lock` builds fine locally and passes both of these, then fails every release build at `cargo build --locked`. That cost a full release attempt: the `v0.1.3` producer run failed closed on it. CI must catch a stale lock before a release does.

Concretely: bump `version` in `[workspace.package]` to `0.1.6`, forget to run `cargo update`/`cargo generate-lockfile`. `cargo build` locally is perfectly happy — it just rewrites the lock. `cargo clippy` passes. `cargo test` passes. The tree is green everywhere, and the release workflow, which runs `cargo +1.89.0 build --release --locked …`, fails closed on *every one of the five targets*, after preflight and resolve have already spent their time. The v0.1.3 producer run is the receipt for that cost. `--locked` on the clippy and test steps converts a release-time discovery into a push-time one.

The related failure has the same shape and is why `check_release_tag.py` exists: v0.1.4 staged binaries that reported `0.1.3` because the version bump was missed entirely, which hosted qualification correctly refused with `installed-daemon-version: daemon reports '0.1.3', expected 0.1.4`.

## The `release-drift` job

```yaml
  release-drift:
    # Eggpack is producer authority for the release matrix: the generated
    # release workflow must be exactly what the pinned tool renders from the
    # checked-in static configuration. This job never builds, publishes, or
    # stages anything; it only fails when the checked-in bytes drift.
    runs-on: ubuntu-latest
```

Two steps. The first installs the pinned tool, deriving the revision from the checked-in policy rather than duplicating it:

```bash
revision="$(python3 -c "import json,sys;sys.stdout.write(json.load(open('release/eggpack/github-policy.json'))['eggpack_tool']['revision'])")"
cargo install --git https://github.com/eggstack/eggpack --rev "$revision" --locked eggpack-cli
eggpack --version
```

The revision is currently `32a0903936fcc283863e0bfb86151b13b4d75ce9`, the same value hard-rendered into every `Install pinned Eggpack tool` step of the generated workflow. Reading it from the policy file is what keeps the two in agreement by construction: change the policy, regenerate the workflow, and the drift job follows automatically.

The second step is the whole check:

```bash
eggpack ci check \
  --workflow-shape release/eggpack/workflow-shape.json \
  --contract release/eggpack/distribution.toml \
  --github-policy release/eggpack/github-policy.json \
  --workflow .github/workflows/release.yml
```

Four inputs: the checked-in workflow, the workflow-shape spec, the distribution contract, and the GitHub policy. The job installs a compiler toolchain but never invokes `cargo build`. It creates no tag, no draft, no artifact, and holds `contents: read`. It cannot publish even in principle.

**The contrast with the workflow it validates** is the point. `release-drift` is hand-written and lives in `ci.yml`; `release.yml` is generated and lives in the repository only as rendered output. `release-drift` is therefore not a second release matrix — it is a checksum on the first. And because `release.yml` is producer authority, a hand edit to it is *not* something a human can "just fix": it fails the drift job, and the only repair is to change `release/eggpack/` and re-render with the pinned tool. `CONTRIBUTING.md` states the procedure as `eggpack ci generate …` followed by `eggpack ci check …`, with the install command that guarantees the same revision is used locally.

## Generated release workflow

`.github/workflows/release.yml` is **generated** by `eggpack ci generate` from `release/eggpack/`. Never hand-edit it. Nine checked-in static inputs feed it: `build-bindings.toml`, `consumer-validators.json`, `distribution.toml`, `github-policy.json`, `github-template.json`, `install-policy.toml`, `pack.toml`, `qualification-bindings.toml`, `workflow-shape.json`.

The rendered job graph is a four-stage fan-out/fan-in:

```text
preflight
   └── resolve                        (one runtime release identity, uploaded as an artifact)
          ├── build_aarch64_apple_darwin        (macos-14)
          ├── build_aarch64_unknown_linux_gnu   (ubuntu-24.04-arm)
          ├── build_x86_64_apple_darwin         (macos-15-intel)
          ├── build_x86_64_pc_windows_msvc      (windows-latest)
          ├── build_x86_64_unknown_linux_gnu    (ubuntu-latest)
          │        └── qualify_build_<target> ── validate_build_<target>  (native, per target)
          └── required_gate  ← all five validate_build jobs
                 └── aggregate
                        └── stage               (ubuntu-latest, permissions: contents: write)
```

`resolve` requires the `release_tag` dispatch input to be non-empty, installs the pinned tool, and runs `eggpack ci _resolve-release` with `--selected 'linux-x64,linux-arm64,macos-x64,macos-arm64,windows-x64'`, `--tag`, `--source-revision` (the checkout's `HEAD`), and the full set of static config paths. Its output — `release-plan.json`, `release-ci-plan.json`, `github-draft.json` — is uploaded as `eggpack-runtime-identity` and downloaded by every downstream job, so all 22 jobs act on one identity rather than re-deriving it.

Each build job checks out the exact dispatched ref, sets up `1.89.0` plus its one target, installs the pinned tool, runs `eggpack ci _verify-source --release-plan …`, then builds with `CARGO_TARGET_DIR` isolated per run attempt:

```bash
'cargo' '+1.89.0' 'build' '--release' '--locked' '--target' 'aarch64-apple-darwin' '--package' 'eggwork-server' '--bin' 'eggworkd'
```

The two Linux targets also build `'--package' 'eggwork-sandbox-helper' '--bin' 'eggwork-sandbox-helper'`; the helper ships on Linux only. Each build then runs `eggpack ci _capture-build` and uploads a canonical handoff. Qualification and validation run natively per target, not from a cross-compiled artifact.

**Staging-only posture.** `stage` is guarded by `if: github.event_name == 'workflow_dispatch'`, holds the repository's only `contents: write`, downloads the finalized release, runs `_prepare-stage` and then `_stage-github-draft`, and uploads a staging receipt. It creates a **draft**. It does not publish, does not create or move a tag, and does not overwrite a differing asset — a same-tag rerun re-reconciles the exact remote asset set and fails closed on a digest difference. The draft template in `release/eggpack/github-template.json` marks the release draft-only, and `release_contract.rs` asserts all three properties. Every other job in the file holds `contents: read`.

Other rendered properties worth knowing: `concurrency` is keyed on `eggpack-${{ github.workflow }}-<tag or ref>` with `cancel-in-progress: true`; every job has `timeout-minutes: 90` (30 for the tool install) and `continue-on-error: false`; artifact retention is 7 days; all four actions are pinned by immutable commit SHA via `github-policy.json`.

**The absence of `RUSTFLAGS` in any workflow is a requirement, not an omission.** `.cargo/config.toml` states the rule and the reason: "No workflow or environment may set `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, or `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS`. Cargo gives the environment precedence over `target.*.rustflags`, so such a setting would silently replace this policy rather than extend it." A reviewer adding a `RUSTFLAGS` env to a build job to "just fix" a Windows build would restore wall-clock timestamps and the CodeView record, and nothing would fail — which is why `release_contract.rs::no_workflow_or_environment_can_replace_the_target_scoped_rustflags` and the environment scans in the reproducibility workflow both exist.

## Windows reproducibility proof

The motivating incident is recorded in both `.cargo/config.toml` and the workflow header: draft `v0.1.0` (release id `400502116`) proved that one exact source revision does not produce one exact Windows binary. Same-tag rerun `36868105194` produced a different SHA-256, a different PE COFF timestamp, and a different CodeView/PDB RSDS signature (`19ab1c9c…` staged vs `cd86a174…` in the rerun). Eggpack correctly refused to replace the same-name asset. The nondeterminism was Eggwork's, not the producer's.

### The two link flags

```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "link-arg=/BREPRO", "-C", "link-arg=/DEBUG:NONE"]
```

- **`/BREPRO`** — MSVC `link.exe` stamps the PE COFF header with wall-clock time unless this flag is given. Two builds minutes apart therefore differ in bytes *alone*, independent of any code change. The flag pins the timestamp to a deterministic value derived from the input.
- **`/DEBUG:NONE`** — debug information is emitted into a CodeView (RSDS) record whose GUID and age come from a *freshly generated PDB*, so the record differs on every build even when the code is byte-identical. This is the second, subtler cause: identical section sizes, identical image size, different bytes.

### The deliberate tradeoff

Release Windows binaries carry no PDB and no CodeView debug directory. Symbols for a shipped release are therefore unavailable from the release artifact itself; crash triage of a released Windows build must rely on a locally rebuilt binary from the same revision. That is accepted because, as `.cargo/config.toml` puts it, "a release that cannot be reproduced byte-for-byte cannot be reused, verified, or re-staged across a same-tag rerun, which is a stronger distribution property than embedded release symbols."

### Target-scoped, not environment

The flags live under `[target.x86_64-pc-windows-msvc]` and nowhere else. Linux and macOS builds are byte-stable under their own toolchains and must not inherit MSVC link arguments. The scoping is load-bearing rather than tidy: Cargo resolves the *environment* ahead of `target.*.rustflags`, so a `RUSTFLAGS` set anywhere — a workflow `env:`, a shell export, a developer profile — would replace the policy wholesale and silently reintroduce both nondeterminism sources. Three independent mechanisms defend against it: `release_contract.rs::the_windows_msvc_build_policy_is_deterministic_and_target_scoped` (which also fails on host-wide `rustflags` in `.cargo/config.toml`), `release_contract.rs::no_workflow_or_environment_can_replace_the_target_scoped_rustflags`, and the runtime environment scans in the workflow and the verifier.

### The workflow and the verifier

`.github/workflows/windows-reproducibility.yml` is `workflow_dispatch` only, `contents: read`, `windows-latest`, 90 minutes. Its header states the posture: "It is deliberately NOT a second producer release matrix: it builds the Windows daemon only, on one host, twice. It does not stage, publish, or mutate any tag."

1. Checkout the exact dispatched revision (`github.sha`) and record `git rev-parse --verify HEAD^{commit}` plus the workspace version.
2. Set up `1.89.0` with target `x86_64-pc-windows-msvc`.
3. **Require that nothing overrides the target-scoped deterministic link policy** — a pwsh step that fails on any environment variable matching `RUST.?FLAGS`, requires `.cargo/config.toml` to exist, and echoes the `BREPRO` and `DEBUG:NONE` lines as evidence.
4. **Build twice into two independent target directories** (`determinism-one`, `determinism-two`), so the second build cannot reuse first-build objects.
5. **Prove byte-identity and absence of CodeView** via `python3 scripts/verify_windows_reproducibility.py <one>/x86_64-pc-windows-msvc/release/eggworkd.exe <two>/…/eggworkd.exe`.
6. **Require version coherence**: run `eggworkd.exe version`, parse the JSON, and require `version` to equal `[workspace.package].version`.
7. Upload a bounded receipt: `.cargo/config.toml` and the verifier script, 7-day retention.

`verify_windows_reproducibility.py` checks the resulting property directly instead of trusting the flags. `sha256_file()` bounds size (`0 < size <= 512 MiB`) and streams the hash. `parse_pe_image()` reads the DOS header, the PE signature, the COFF timestamp, and the debug data directory (index 6), resolves the directory RVA through the section table, and walks 28-byte debug-directory entries looking for `IMAGE_DEBUG_TYPE_CODEVIEW` (2). It fails closed everywhere: no `MZ`, no PE signature, unsupported optional-header magic, out-of-bounds section table, an unnamed section, a debug directory not backed by file data, a size that is not a multiple of the entry size, a CodeView record out of bounds. A non-`RSDS` CodeView variant is **rejected, not tolerated** — the comment is explicit that NB55/NB09 variants embed their own timestamp and are nondeterministic too. `require_byte_identical()` compares digests *and* raw bytes ("identical digests but different bytes"), and reports both digests with both COFF timestamps on mismatch, because the historical failure was exactly a timestamp-plus-CodeView difference. `require_no_codeview()` exists because a surviving CodeView record means the deterministic policy did not apply — either the rustflags were replaced or they were never present. `check()` scans the environment first (spelled by concatenation, `name.upper() == "RUST" "FLAGS" or name.upper().endswith("_RUST" "FLAGS")`, precisely so the string `RUSTFLAGS` never appears as a literal token in the source it is hunting) and returns a one-line summary.

Because this runs before any tag exists, a Windows nondeterminism regression is discovered while it is still a commit rather than a half-staged release.

## Operational qualification workflow

`.github/workflows/operational-qualification.yml` qualifies the *installed* product surface from the exact bytes of one already-staged release. It is not a producer pipeline: no rebuild, no staging, no tag mutation, no second release matrix, only immutable action revisions, and `permissions: contents: read` at the workflow level. The header spells out the token handling: a staged candidate is a draft, the REST API does not expose drafts to the default Actions `GITHUB_TOKEN`, so the harness reads `RELEASE_QUALIFICATION_TOKEN` from a scoped secret — "it never appears in argv, stdout, or a receipt, and the workflow's own `permissions` remain `contents: read`, so the workflow itself cannot create, move, or publish anything even with a broader token available."

Inputs: `release_tag` (required), `prior_release_tag` and `release_id` (optional, for the rollback path).

**`installed-execution`** — a three-way matrix (`fail-fast: false`): `ubuntu-latest` / `x86_64-unknown-linux-gnu` / linux, `macos-14` / `aarch64-apple-darwin` / macos, `macos-15-intel` / `x86_64-apple-darwin` / macos. Each leg checks out the *qualification tooling* (not a release build), sets up `1.89.0`, then runs `qualify_release.py install`, `qualify_release.py installer`, materialises the bounded fixture, and runs the installed-execution test:

```bash
EGGWORK_QUALIFY_ROOT="…/qualification" \
  cargo test --locked -p eggwork-server --features qualification \
    --test installed_qualification -- --ignored --exact qualification_fixture
…
EGGWORK_QUALIFY_DAEMON: …/qualification/bin/eggworkd${{ runner.os == 'Windows' && '.exe' || '' }}
EGGWORK_QUALIFY_PLATFORM: ${{ matrix.platform }}
  cargo test --locked -p eggwork-server --features qualification \
    --test installed_qualification -- --ignored \
    --exact installed_release_admits_execution_and_refuses_unsupported_isolation --nocapture
```

The fixture comes from the repository's own `rcgen`-based harness, not from whichever OpenSSL or LibreSSL the runner ships — those disagree about certificate version, extensions, and key encodings, and "a certificate the installed binary rejects would look like a product defect rather than a fixture problem." The daemon under test is the one the *installer* placed, driven through the production client.

**`native-service-lifecycle`** — macOS only (`aarch64-apple-darwin` and `x86_64-apple-darwin`, both flagged `unsupported_service: true`): `install`, fixture, `qualify_release.py service`, then `qualify_release.py update --prior-release-tag …` to exercise drain, update, and rollback against real generations.

**`windows-installed-execution`** — a hand-written, non-matrix job, and the most interesting one in the file. The header records two structural gaps found by hosted qualification: the installed `eggworkd` implements no service-control dispatcher, so SCM registers the service and then fails to start it with Windows error 1053; and the runner handed every child a Unix-only baseline environment (`PATH=/usr/bin:/bin`, no `SystemRoot`), so a Windows child started and exited with no capturable exit code. Both were fixed on `main` and neither is in the qualified candidate's bytes. So Windows is **dispositioned, not claimed**: the job records the real disposition and "fails if the observation ever diverges from it, so the limitation cannot silently rot." The pwsh step runs `service install`, writes a receipt, and then:

```powershell
if (-not $unsupported) {
  "DISPOSITION: CHANGED, the service verb mutated state; update the support matrix" | Out-File …
  throw "Windows service install did not refuse; the fail-closed guarantee is gone"
}
if ($exit -eq 0) { throw "expected a non-zero refusal exit, got $exit" }
…
exit 0
```

The trailing `exit 0` is deliberate and commented: the refusal *is* the recorded disposition, so the step must not inherit the daemon's nonzero exit as its own result. A release that registers a service has lost the guarantee, and that is a hard failure, not a note.

Every job uploads **bounded receipts** — `${{ runner.temp }}/receipts/*.json` (plus `windows-service.txt`) under a run-id-and-attempt-suffixed artifact name, `if-no-files-found: error`, 7-day retention. The harness internals — token handling, retry policy, size bounds, stage sequencing — are covered in [release-qualification.md](release-qualification.md).

## Lints and toolchain policy

The workspace root declares:

```toml
[workspace.lints.rust]
unsafe_code = "deny"

[workspace.lints.clippy]
all = "warn"
```

All five crates — `eggwork-core`, `eggwork-runner`, `eggwork-client`, `eggwork-server`, `eggwork-sandbox-helper` — opt in with `[lints] workspace = true`, so the policy is declared once and cannot be weakened per crate without an obvious manifest diff.

`all = "warn"` sets the floor; `-D warnings` on the clippy step supplies the promotion. The two together are what make the table load-bearing: `all = "warn"` alone would accumulate warnings indefinitely, and `-D warnings` alone would only cover the crates that declare lints.

**`unsafe_code = "deny"` is a hard workspace-level guarantee, and it matters disproportionately for this codebase.** Eggwork's isolation story is Landlock plus `no_new_privs` plus cgroup limits plus rlimit wrappers, applied by `eggwork-sandbox-helper` before the target is launched. A confinement mechanism is only as strong as its weakest component; a single `unsafe` block in the server, client, or core could bypass the runner's entire admission path without any visible architectural change. Denying it workspace-wide, with no per-crate exception mechanism, means "no unsafe" is a property of the repository rather than a code-review habit. The same reasoning underlies the process-ownership guard: both exist because the alternative is a silent, unreviewable bypass of a guarantee that is otherwise carefully designed.

`rust-toolchain.toml` pins the tool:

```toml
[toolchain]
channel = "1.89.0"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

Both `ci.yml` and the generated release workflow install the same `1.89.0` channel, and the release builds invoke `cargo +'1.89.0'` explicitly. `minimal` keeps the profile small; the two components are exactly what CI needs. `release_contract.rs::toolchain_policy_matches_the_checked_in_rust_toolchain` asserts the checked-in policy matches what the release configuration expects.

`[workspace.dependencies]` centralizes every third-party version and feature selection in one table (`serde`, `tokio`, `rusqlite` with `bundled`, `landlock = "=0.4.7"` pinned exactly, `eggserve-core`, `eggfetch-core`, `eggress-outbound`, …), and each crate inherits with `foo.workspace = true` or adds only local features. This is what makes a single `--locked` lockfile meaningful: there is exactly one place where a version or a feature set can change, and feature unification across the workspace is visible in one diff. `eggwork-server` declaring `tokio = { workspace = true, features = ["net", "signal"] }` — with `process` conspicuously absent — is the manifest-level expression of what `check_dependencies()` asserts.

## Other guard scripts

**`scripts/check_release_tag.py` (pre-tag guard).** Takes one argument, the exact tag about to be created, e.g. `python3 scripts/check_release_tag.py v0.1.5`. It requires three things to agree: the tag (minus a leading `v`) equals the first `version = "…"` in `Cargo.toml`; every `eggwork-*` package in `Cargo.lock` is pinned at that same version; and the run is otherwise clean. Its docstring names both incidents it exists for — v0.1.3 (bump without lock regeneration, so every `--locked` release build failed closed) and v0.1.4 (bump missed entirely, so staged binaries reported `0.1.3` and hosted qualification refused them with `installed-daemon-version: daemon reports '0.1.3', expected 0.1.4`). Crucially, it is **not** a CI check, and the docstring explains why: "The tag-to-version link cannot live in CI: the generated release workflow is producer authority (the `release-drift` job rejects hand edits to it), and CI never knows the future tag. So the check lives here, run by whoever cuts the tag, before the tag exists." It defends the tag→version→lock identity, and its failure message ends with the remediation: `bump Cargo.toml, regenerate the lock, rerun the gates, then tag.`

**`scripts/release_candidate_probe.py` (shared bounded probe library).** Not a validator itself — the shared substrate the two Eggpack-facing validators import. Eggpack invokes each validator as `python3 <script> <candidate>` from the checked-out repository root, and "Nothing else is accepted: there is no release index, no network, no shell, and no ambient Eggwork, Eggup, or service-manager consultation." Its bounding constants are the substance: `MAX_SOURCE_BYTES = 1 << 20`, `MAX_VERSION_BYTES = 64`, `MAX_CANDIDATE_STDOUT`/`STDERR = 64 * 1024`, `CANDIDATE_TIMEOUT_SECONDS = 30`. `run_candidate()` refuses a non-regular file *or a symlink* (`candidate.is_symlink()`), passes `stdin=DEVNULL`, `shell=False`, the fixed argv, and a timeout; it raises `ValidationFailure` on timeout, `OSError`, over-bound output, or nonzero exit. `candidate_environment()` is an **allowlist** — `("PATH", "HOME", "TMPDIR", "LANG", "LC_ALL")` on POSIX, `("SYSTEMROOT", "SystemRoot", "COMSPEC", "PATHEXT", "WINDIR")` on Windows. `workspace_version()` reads `[workspace.package].version` with `tomllib` under a size bound and fails closed on a malformed or unbounded manifest. The security-relevant property is stated at the top: "Candidate output is deliberately never returned. Eggpack records only the validator's exit status, and these helpers print fixed reason phrases so a failing run cannot leak candidate bytes into workflow logs or release evidence." It defends every release surface Eggpack gates on, by being the thing that cannot leak, hang, or shell out.

**`scripts/validate-daemon-version.py` (Eggpack consumer validator, daemon).** Eggpack selects the daemon candidate as the direct artifact of a macOS or Windows target, or as bundle entry 0 of a Linux target, and runs this after core qualification. `check()` runs `eggworkd version`, requires the output to parse as a JSON **object** (`daemon did not report version JSON` / `daemon version output is not a JSON object`), requires a non-empty string `version` field (`daemon version output has no version field`), and requires it to equal `expected_version_line()` (`daemon version differs from the workspace package version`). It turns "the candidate exits zero" into a version-coherence claim *on every required target*, not only on Linux. It defends the direct and bundle-entry-0 daemon artifacts of all five targets.

**`scripts/validate-sandbox-helper.py` (Eggpack consumer validator, Linux helper).** Eggpack selects the helper as bundle entry 1 of a Linux target and runs this after the daemon's core qualification. `check()` runs `eggwork-sandbox-helper --version` and requires **exactly one** non-empty line (`helper did not report exactly one version line`, `helper reported an empty version line`) equal to the workspace version. The docstring names the payoff: "That is the same identity `deployment::check_helper_compatibility` requires at update time, so a helper whose version drifts from the daemon is refused before the release can ship." So this defends the Linux helper bundle member by pre-checking the exact invariant the update path will later enforce at install time.

Both validators are invoked by **Eggpack**, inside the generated release workflow's `validate_build_*` jobs — not by `ci.yml` and not by the qualification harness. `check_release_tag.py` is invoked by **no workflow at all**; it is a human pre-tag step. `verify_windows_reproducibility.py` is invoked by `windows-reproducibility.yml`. `qualify_release.py` is invoked by `operational-qualification.yml` (its `install`, `installer`, `service`, and `update` stages) and is covered in [release-qualification.md](release-qualification.md); its `main()` also exposes `inventory` and a `--public` anonymous-read mode, and defaults `--release-tag` to `f"v{probe.workspace_version()}"` rather than a hard-coded tag, so a stale default qualifies the previous release only if the workspace version is genuinely stale.

## Python test suite

```bash
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
```

The `--top-level-directory` pin matters: it makes the modules importable as top-level names without depending on the repository root being on `sys.path`, so discovery behaves the same locally and in the `rust` job. There is no Python test runner, no `pytest`, and no dependency manifest — the guard layer's tests are stdlib-only, which is what lets them run in the same job as the Rust suite with no setup step.

**`tests/release/test_qualify_release_harness.py`** — the harness's *network* behavior, driven against a fake `_Response`. `AssetDownloadTest` covers the retry and bound policy: writes the asset, retries a transient 5xx, retries a transient rate limit, retries a transport error, does **not** retry a client error, fails loudly when retries are exhausted, and refuses an oversized asset *without* retrying. `PublicReadTest` covers the token/public distinction that the operational workflow depends on: a tokened read cannot masquerade as public, a public read needs no token, a **draft read still requires a token**, and public is rejected for the privileged stages. That last pair is the regression test for the exact architectural claim in the workflow header — drafts are invisible to the default `GITHUB_TOKEN`.

**`tests/release/test_release_candidate_probe.py`** — the validators. `ValidatorTest` covers both validators in both directions: the helper accepts the workspace version and rejects a different one, an empty line, more than one line, the daemon as a substitute, a nonzero exit, and non-UTF-8 output; the daemon accepts the workspace version and rejects a different one, a missing `version` field, the helper as a substitute, and non-JSON output. It also rejects an unusable candidate and a **symlinked** candidate. `EntrypointTest` pins the two-argument contract and that a failure reports a fixed reason *without* candidate output — the no-leak property. `WorkspaceVersionTest` asserts fail-closed behavior on a malformed manifest, a missing package version, and an out-of-bounds value. `BuiltCandidateTest` runs the *actually built* helper and daemon when they are present, so the validators are checked against real binaries rather than only stand-ins. `BoundednessTest` is the meta-test: the workspace version matches the checked-in manifest, **the probe never shells out**, **the probe never reaches the network**, the candidate environment is an allowlist, and the validators take only the candidate path. These are source-level assertions about the probe itself, and they are the reason the "no shell, no network" claims in its docstring are testable.

**`tests/release/test_windows_reproducibility.py`** — the PE parser, off-Windows. `PeParsingTest` reads a hand-built PE fixture: the COFF timestamp "that the historical mismatch turned on," the RSDS record a debug build carries, the absence of any CodeView record under `DEBUG:NONE`, the rejection (not tolerance) of non-RSDS CodeView variants, and fail-closed behavior for unparseable input, an unsupported optional-header magic, and a debug directory pointing outside file data. `PreflightTest` covers the property checks: two identical stripped builds pass; a timestamp-only difference is reported with **both** timestamps; a CodeView difference alone fails; `require_no_codeview` rejects a debug build and accepts a `DEBUG:NONE` one; `check` rejects an environment `RUSTFLAGS` override and passes with no override; and out-of-bounds or missing inputs fail closed.

**How they complement the Rust suite.** They test the *harness*, not the product. `cargo test` asserts what the daemon and server do; these assert that the instruments measuring them are not lying — that the downloader retries what it should and not what it should not, that the probe cannot shell out or reach the network or leak bytes, that the PE parser fails closed on images it does not fully understand, and that an environment override of the deterministic policy is rejected rather than absorbed. A green Rust suite with a broken qualifier is a release that looks qualified and is not.

## Pre-submit discipline

`CONTRIBUTING.md` opens with the boundary statement: "Changes must preserve the scheduler-free fixed-target boundary and crate ownership described in `architecture/`. Run formatting, Clippy, workspace tests, workspace check, and `python3 scripts/check_execution_ownership.py` before submitting Rust changes. Production process creation belongs only to `eggwork-runner`, with the documented sandbox-helper exception. Planning changes must follow `plans/003-planning-process.md`."

The command list, in order, with the purpose of each:

| # | Command | Purpose |
| --- | --- | --- |
| 1 | `python3 scripts/check_execution_ownership.py` | Prove singular process ownership and crate dependency direction before anything else runs. |
| 2 | `python3 scripts/check_execution_ownership.py --prove-negative-exit` | Prove the guard still fails when it should — the step CI also runs. |
| 3 | `cargo fmt --all -- --check` | Formatting; keeps review attention on substance. |
| 4 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Lint cleanliness at the same coverage CI uses. |
| 5 | `cargo test --workspace --all-targets` | The Rust suite, including the 36 release-contract tests. |
| 6 | `cargo check --workspace` | Fast whole-workspace type check. |
| 7 | `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | The guard-layer test suite. |
| 8 | `git diff --check` | Whitespace errors and conflict markers; the one command CI does not run. |

Note that the local list is the CI list minus `--locked` and plus `git diff --check`. Add `--locked` to steps 4 and 5 locally when you have touched the manifest or the lockfile, which is the case where the difference actually bites.

### Release configuration changes

Eggpack is the producer authority. `release/eggpack/` is the checked-in configuration and `.github/workflows/release.yml` is generated from it, "so never edit the workflow by hand." After changing anything under `release/eggpack/`:

```bash
revision="$(python3 -c "import json,sys;sys.stdout.write(json.load(open('release/eggpack/github-policy.json'))['eggpack_tool']['revision'])")"
cargo install --git https://github.com/eggstack/eggpack --rev "$revision" --locked eggpack-cli

eggpack ci generate \
  --workflow-shape release/eggpack/workflow-shape.json \
  --contract release/eggpack/distribution.toml \
  --github-policy release/eggpack/github-policy.json \
  --output .github/workflows/release.yml

eggpack ci check \
  --workflow-shape release/eggpack/workflow-shape.json \
  --contract release/eggpack/distribution.toml \
  --github-policy release/eggpack/github-policy.json \
  --workflow .github/workflows/release.yml
```

Install the *exact pinned revision* read from `github-policy.json` — not `main`, not a remembered SHA. The `check` is the local echo of the `release-drift` job, so a change that passes locally passes CI for the same reason.

Two further hard rules. First: "Never commit a release tag, source SHA, artifact digest, or artifact size into `release/eggpack/`. The static configuration is identity-free by design; CI fails the build if it is not." Second: "Adding a target or changing an artifact form also requires updating `crates/eggwork-server/tests/release_contract.rs`, which reconciles the producer configuration against Eggwork's deployment install unit." The second is the one that is easy to skip: adding a sixth target without extending the contract test leaves the two authorities disagreeing silently, and the reconciliation is exactly what that file exists to do.

### Cutting a release

Land and tag, push the tag, then run the **Release** workflow with `release_tag` set to that exact existing tag. "There is no latest/branch fallback; a run refuses a tag that does not exist and every job re-verifies the checked-out `HEAD` against the resolved release plan." The run builds and natively qualifies all five targets, gates on required evidence, finalizes with checksum sidecars and a `ReleaseManifest`, and stages a draft. Then: "Inspect the draft, then publish it. Publication is a human action." A rerun of an already-staged release re-reconciles the exact remote asset set and fails closed if a digest differs, rather than overwriting it.

### What CI does *not* run that a careful contributor should

- **`git diff --check`** — whitespace and conflict-marker hygiene, item 8 above.
- **`scripts/check_release_tag.py <tag>`** — cannot be in CI, because CI does not know the future tag. Run it before pushing a tag.
- **`windows-reproducibility.yml`** — `workflow_dispatch` only. If you touched the Windows build path, `.cargo/config.toml`, the toolchain, or anything that changes link inputs, dispatch it yourself. A green `rust` job says nothing about Windows byte-reproducibility.
- **`operational-qualification.yml`** — also `workflow_dispatch`. It needs a staged release, so it cannot precede a commit, but it is the only thing that exercises the *installed* surface on three hosted operating systems.
- **The full `--locked` matrix** — local clippy and test run without `--locked` by default. If the manifest or lockfile changed, run them with it.
- **A `--public` anonymous qualification read** — the harness's `inventory` stage can read a published release exactly as a real consumer would. `CONTRIBUTING.md` does not require it pre-submit, but the token/public boundary is a release property worth exercising once.

## Test coverage and gaps

**What is genuinely verified.** Singular process ownership, as a static property over `crates/*/src/**/*.rs`, with the guard's own failure path continuously tested. Crate dependency direction, from real `cargo metadata`. Lint and formatting cleanliness at full workspace and feature coverage, promoted to hard failures. Tag/version/lock coherence, at the moment the tag is cut. Identity-freedom of the release configuration, and the derivation of the release workflow from it. Deterministic Windows linkage, both statically and natively by two-build byte-identity. Installed-product behavior on hosted Linux, macOS, and Windows, with receipts. Fail-closed dispositions for the platform claims the product does *not* make.

**What is not, stated honestly.**

- **The ownership guard is a static, pattern-based check, not a proof.** It matches four textual patterns over source with comments and literals stripped. It is blind to: process creation through any API the patterns do not name; process creation in `crates/*/tests/`, `examples/`, `benches/`, or `build.rs`; indirect creation through a helper function whose name is not in the list; FFI to a C library that forks; and any code generation that produces a spawn at build time. It proves the *named idioms* are confined to the approved crates. It does not prove that no other path creates a process.
- **The dependency-direction check is an enumeration, not a graph proof.** It inspects two crates against a fixed forbidden set and one positive requirement. It does not compute a layering, does not detect a new crate that creates processes and is depended upon by the server, and does not check dev-dependencies separately from normal dependencies. A new crate is checked only if it happens to appear in one of those intersections.
- **The reproducibility proof covers Windows only.** The same revision is not proven byte-reproducible for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, or `aarch64-apple-darwin`. `.cargo/config.toml` asserts those are byte-stable under their own toolchains, and Eggpack's same-name digest/no-clobber behavior would catch a mismatch — but only *after* a same-tag rerun, which is the expensive discovery this layer exists to avoid.
- **Hosted qualification proves a subset of platform claims.** It runs on `ubuntu-latest`, `macos-14`, `macos-15-intel`, and `windows-latest` — not on every architecture of every platform, not on any non-x86_64 Linux with a different libc, and not on any Linux distribution other than the hosted image. `execution-ownership.md` states that Linux was exercised for this milestone and that non-Unix hosts return an explicit unsupported-platform error.
- **No invocation of any workflow in this document runs on `pull_request` except `ci.yml`.** Every qualification, reproducibility, and release run is `workflow_dispatch`. These are expensive, secret-bearing, and evidence-producing; they are run deliberately, not automatically. The cost is that a real regression can sit green in `main` until someone dispatches a run.
- **The Windows determinism policy is a flag-and-observe pairing, not a guarantee.** It is enforced only when `windows-reproducibility.yml` is dispatched. `release_contract.rs` can assert the flags are present and target-scoped; it cannot assert the linker honored them. The native two-build proof is the only real check, and it is manual.
- **`ci.yml` pins `actions/checkout@v4`, `dtolnay/rust-toolchain@1.89.0`, and `Swatinem/rust-cache@v2` by mutable tag**, while the generated release workflow pins all four of its actions by immutable commit SHA. CI therefore has a supply-chain posture weaker than the release path, and `github-policy.json` does not govern it.
- **The token path in the qualification workflow is trusted, not verified here.** The claim that `RELEASE_QUALIFICATION_TOKEN` never appears in argv, stdout, or a receipt is asserted in the workflow header and in the harness docstring; this layer does not independently test the workflow's own environment handling.
- **No guard checks for direct `systemctl` / `launchctl` / SCM / `crontab` invocation.** Invariant 2 rests on review plus the Windows disposition job, not on a mechanical check. A `systemctl` call in a non-approved crate would not trip the ownership guard, because `systemctl` is not one of the four spawn patterns — it would be a `Command::new("systemctl", …)`, which *is* one of them, and so the guard does cover the ordinary spelling. The gap is a *direct* binding, or a new process-owner crate that is allowlisted.

## Review focus

1. **Allowlist growth.** Every added entry in `APPROVED_PROCESS_OWNERS` moves an architectural boundary. Does the diff include the rationale in `execution-ownership.md` and both a negative and a positive fixture, in the same change? A bare entry with no doc update is the easiest possible way to fork process ownership in this repository.
2. **New spawn idioms.** The scanner matches four patterns plus one alias form. Does the change use `libc::fork`, `posix_spawn`, `nix::unistd::exec*`, a renamed wrapper (`run_tool`, `launch`, `start_sidecar`), or code generation? Any of these defeats the guard silently.
3. **Scan coverage.** The glob is `crates/*/src/**/*.rs`. A new `crates/*/src/bin/…` binary, an `examples/` target, or logic moved into `build.rs` is unscanned. Is production code going where the guard can see it?
4. **Dependency-direction completeness.** The forbidden sets are hard-coded for `eggwork-core` and `eggwork-client`. What happens when a sixth crate appears? Does it get a direction rule, and is `eggwork-server`'s positive `eggwork-runner` requirement still the right shape?
5. **Could `--prove-negative-exit` pass vacuously?** It proves `scan_file_for_spawns` flags a synthetic file; it does **not** exercise `main()`'s error aggregation, `scan_sources()`, or `check_dependencies()`. A refactor that made `scan_sources()` silently skip everything would still pass this proof. Is that acceptable, and should the proof reach the aggregate path?
6. **Workflow and pinned-tool drift.** `ci.yml` uses mutable action tags while the release workflow uses immutable SHAs. `release-drift` installs the pinned Eggpack revision from `github-policy.json`; the generated workflow hard-renders the same SHA. Do they still agree, and should `ci.yml` adopt the immutable form?
7. **Could any workflow reintroduce `RUSTFLAGS`?** Search all four workflows for `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS`, and any shell `export` of a similarly named variable. Remember that Cargo's environment precedence means one `env:` line silently replaces `.cargo/config.toml`. The three existing defenses cover the checked-in files; a new workflow is the gap.
8. **Secret exposure in CI logs.** `RELEASE_QUALIFICATION_TOKEN` is passed as a step `env:` to several `python3` invocations and to `cargo test`. Confirm it cannot reach argv (visible in `ps` and in error echoes), a receipt, or `--nocapture` output. The harness's own no-leak discipline covers candidate bytes, not the token.
9. **Python coverage of the harness's fail-closed paths.** `test_windows_reproducibility.py` and `test_release_candidate_probe.py` do test fail-closed behavior directly. `test_qualify_release_harness.py` covers retry and token/public policy but is the least exhaustive of the three — are there unreviewed failure paths in `_resolve_release` or the stage transitions?
10. **Windows determinism regression risk.** `/DEBUG:NONE` and `/BREPRO` are load-bearing for a distribution property that cost a full release attempt. Does any change to the build path, toolchain, or MSVC version plausibly reintroduce a COFF timestamp or a CodeView record? If so, was `windows-reproducibility.yml` dispatched?
11. **The published-symbol tradeoff.** Release Windows binaries deliberately carry no PDB. Any change to crash reporting, symbolication, or field diagnostics should be checked against that accepted loss before it silently reintroduces nondeterminism to solve it.
12. **Contract test completeness.** `release_contract.rs` is 36 tests and is the primary static enforcement for invariants 6, 7, and 8. When a target or artifact form changes, is the contract test extended in the same change, and does `install_identities_match_the_deployment_install_unit` still hold against the deployment install unit?

## Related

- [overview.md](overview.md) — crate map, request flow, and the cross-component invariants this layer enforces
- [execution-ownership.md](execution-ownership.md) — the normative rule `check_execution_ownership.py` implements
- [release-qualification.md](release-qualification.md) — the qualification harness internals this workflow drives
- [distribution.md](distribution.md) — the producer/consumer split Eggpack owns
- [deployment-lifecycle.md](deployment-lifecycle.md) — the install unit the release contract test reconciles against
- [operations-cli.md](operations-cli.md) — the operator surface the harness exercises through real commands
- [runner-execution.md](runner-execution.md) — the process lifecycle whose ownership the guard protects
- [sandbox-helper.md](sandbox-helper.md) — the second approved process owner and the isolation it applies
- [../CONTRIBUTING.md](../CONTRIBUTING.md) — the pre-submit checklist and release procedure
- [../SECURITY.md](../SECURITY.md) — reporting and disclosure
