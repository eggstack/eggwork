# 004b — Staged helper publication durability corrective

Status: **closed**
Source roadmap: `plans/subsystems/operations-distribution-roadmap.md` (M004b)
Corrects: `plans/closure/operations-distribution/004a-status.md` §5, §9 criterion 3, §10, §11
Closure record: `plans/closure/operations-distribution/004b-status.md`
Branch: `planning/optional-gregg-telemetry-m001-m002`

## 1. Objective

Eliminate the intermittent `ETXTBSY` refusal that the Operations M004a
closure characterised but did not close, so that a Linux suite which stages a
helper and immediately executes it cannot go red at random.

## 2. Baseline and problem

`ETXTBSY` ("Text file busy") is Linux refusing to execute a file whose inode
still carries an open write reference. The M004a closure recorded the refusal
as a host-level timing window, dispositioned rather than fixed, and left the
elimination to a future `O_TMPFILE`+`linkat` publication that the repository's
rules forbid.

CI run `39306109740` then failed on that open residual:
`malformed_private_spec_and_unavailable_status_channel_do_not_launch_target`
panicked at `crates/eggwork-sandbox-helper/tests/landlock_runner.rs:471` with
`ExecutableFileBusy`. A characterisation that leaves the suite able to go red
is not a disposition.

## 3. Scope

- The shared helper-staging path in
  `crates/eggwork-sandbox-helper/tests/support/helper_fixture.rs`.
- The equivalent staging helper in `crates/eggwork-server/src/lib.rs`
  (`place_trusted_helper`, inside the test module).
- Two regression tests in `crates/eggwork-sandbox-helper/tests/landlock_runner.rs`.
- The closure record, the roadmap entry, and the registry rows.

## 4. Invariants and non-goals

**Invariants**

1. The staging path keeps its shape: build output is read-only input, each
   fixture owns its own `0700` directory and its own path, publication is
   atomic, and cleanup follows convergence.
2. The fix is structural — a file that is never written is executable — not a
   retry and not a sleep.
3. No fixture mutates an inode it shares with another fixture or with the
   Cargo-built helper.
4. No `unsafe`, no new dependency, no new platform surface, no change to
   production behaviour.
5. The M004a closure record is preserved as written.

**Non-goals**

- Changing `verify_trusted_helper` or the trust rules.
- Changing the fixture-isolation invariant or weakening the M004a proofs.
- `O_TMPFILE`+`linkat` publication.
- Changing how the product installs or consumes the helper.

## 5. Required changes

1. `helper_fixture.rs`: `stage` **links** the Cargo-built helper into the
   fixture's private directory rather than copying it, so the file the fixture
   executes is never written and the refusal cannot arise. When a link is
   impossible — a different filesystem, or a source that is not executable — it
   falls back to a private copy that is committed before publication.
2. `helper_fixture.rs`: `set_mode` and `replace_with_junk` publish a
   **replacement** instead of changing the inode, because the staged helper may
   be a link shared with the build output and with other fixtures.
3. `helper_fixture.rs`: expose `stage_private_copy` so the write-and-commit
   path can be exercised directly rather than only through a fallback nobody
   takes.
4. `eggwork-server/src/lib.rs` `place_trusted_helper()`: the same link-or-
   committed-copy behaviour.
5. `landlock_runner.rs`: two regression tests — one for the link path with the
   real helper, one for the copy path.

## 6. Required tests

| Test | Requirement |
| --- | --- |
| `a_linked_helper_is_executable_immediately_after_publication` | 32 parallel stage-and-spawn cycles of the real helper; zero refusals. Must **fail** when `stage` is forced back through an unbarriered copy — verified, not asserted. |
| `a_staged_helper_is_executable_immediately_after_publication` | 32 parallel publish-and-spawn cycles of a private copy; zero refusals. Must **fail** when the copy path's barrier is removed — verified, not asserted. |
| the 16 pre-existing Landlock tests | unchanged semantics, unchanged assertions |

## 7. Required verification

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --locked --workspace
python3 scripts/check_execution_ownership.py
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

Plus a bounded repeated run of the Landlock suite at raised thread count, and
both regression tests re-run with the corresponding fix removed, to prove they
are non-vacuous.

## 8. Acceptance criteria

1. The refusal is eliminated by construction: a staged helper is never a file
   the fixture has just written, so the kernel has no reason to refuse it.
2. The elimination is structural — no retry, no sleep, no reliance on timing.
3. Both regression tests are non-vacuous, proven by removing the fix.
4. Production behaviour is unchanged; the change is test-only.
5. No new dependency and no `unsafe`.
6. The M004a isolation invariant still holds: no fixture mutates a shared
   inode.
7. The M004a record is preserved; the correction is registered as current
   authority in the roadmap and the registry.

## 9. Stop conditions

- If eliminating the refusal requires `unsafe`, a new dependency, a change to
  the trust rules, or weakening a fixture-isolation assertion, stop and record
  the alternative for review instead.
- If the refusal cannot be eliminated but only made rarer, record the residual
  as a finding with its measured rate rather than closing the milestone.

## 10. Closure evidence required

The closure record must distinguish inspected from executed evidence, record
the measurement that established the mechanism, show both regression tests
failing without the fix, and state the residual risk honestly.
