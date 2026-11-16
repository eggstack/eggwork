# 004b — Staged helper write-access corrective

Status: **closed**
Source roadmap: `plans/subsystems/operations-distribution-roadmap.md` (M004b)
Corrects: `plans/closure/operations-distribution/004a-status.md` §5, §9 criterion 3, §10, §11
Closure record: `plans/closure/operations-distribution/004b-status.md`
Branch: `planning/optional-gregg-telemetry-m001-m002`

> **Naming.** The milestone was registered as "staged helper publication durability
> corrective", on the hypothesis that the refusal was a write-back problem that a
> durability barrier could close. CI disproved that hypothesis (§2), and the
> executed design is not a barrier at all: the staged helper is never written.
> The milestone ID and the file names are kept so that the pointer from the
> M004a record's addendum keeps resolving, but the design this plan describes
> is the executed one.

## 1. Objective

Eliminate the `ETXTBSY` refusal that the Operations M004a closure characterised
but did not close, so that a Linux suite which stages a helper and immediately
executes it cannot go red at random.

## 2. Baseline and problem

`ETXTBSY` ("Text file busy") is Linux refusing to execute a file whose inode is
open for writing. The M004a closure recorded the refusal as a host-level timing
window, dispositioned rather than fixed, and left the elimination to a future
`O_TMPFILE`+`linkat` publication that the repository's rules forbid.

CI run `39306109740` then failed on that open residual:
`malformed_private_spec_and_unavailable_status_channel_do_not_launch_target`
panicked at `crates/eggwork-sandbox-helper/tests/landlock_runner.rs:471` with
`ExecutableFileBusy`. A characterisation that leaves the suite able to go red is
not a disposition.

The first attempt at this corrective then failed CI in turn, and its failure is
what fixed the design (§3).

## 3. The design change this plan records

The first attempt staged the helper by **copying** it and closed the copy with a
durability barrier — write, `sync_all`, rename, sync the parent directory — on
the theory that the refusal was a write-back window. The barrier was measured
at 0.02% on the development host and was carried to CI.

CI run `39316541191` (and the push run `39316444280`) refused that theory: the
new copy-path regression test was refused 3 times in 32 stagings on the CI
runner, *with the barrier in place*, while the link-path test passed 32 of 32 in
the same run. A barrier that is 0.02% on one host and 9% on another is not a
barrier; it is a rate.

The design that replaced it is stated in §4 and §5, and the evidence for it —
including the kernel's own accounting, which the first attempt had inferred
rather than read — is in the closure record.

## 4. Scope

- The shared helper-staging path in
  `crates/eggwork-sandbox-helper/tests/support/helper_fixture.rs`.
- The equivalent staging helper in `crates/eggwork-server/src/lib.rs`
  (`place_trusted_helper`, inside the test module).
- The regression tests in `crates/eggwork-sandbox-helper/tests/landlock_runner.rs`.
- The closure record, the roadmap entry, and the registry rows.

## 5. Invariants and non-goals

**Invariants**

1. The staging path keeps its shape: build output is read-only input, each
   fixture owns its own `0700` directory and its own path, publication is
   atomic, and cleanup follows convergence.
2. The fix is structural — the file the fixture executes is never written — not
   a retry, not a sleep, and not a barrier.
3. No fixture mutates an inode it shares with another fixture or with the
   Cargo-built helper. The one exception is a one-time, one-directional
   normalisation of the build output's *mode* (see §6.2).
4. No `unsafe`, no new dependency, no new platform surface, no change to
   production behaviour.
5. The M004a closure record is preserved as written.

**Non-goals**

- Changing `verify_trusted_helper` or the trust rules.
- Changing the fixture-isolation invariant or weakening the M004a proofs.
- `O_TMPFILE`+`linkat` publication.
- Changing how the product installs or consumes the helper.

## 6. Required changes

### 6.1 Staging is link-only

`stage(source)` publishes the Cargo-built helper into the fixture's private
`0700` directory with `link(2)`. A link shares the source's inode and does not
open it for writing, so the inode's write access is never taken and the kernel
has no reason to refuse to execute it.

There is **no copy fallback**. A copy is a file that was written, and a written
file is exactly the construction the CI evidence refuses. When a link is
impossible, staging fails loudly with the reason.

The system temporary directory is tried first, because its ancestor chain is the
one `verify_trusted_helper` accepts by construction; a directory next to the
source is the fallback, because it is the same filesystem by definition.

### 6.2 The source's mode is normalised once

A link carries the source's mode, and `verify_trusted_helper` rejects a helper
that anyone but its owner may write. Cargo derives the executable's mode from
the umask (`0666 & ~umask` plus the execute bits), so a permissive umask
(0002 is common in shared development environments) leaves the build output
group-writable and no fixture could stage it by linking.

The fixture therefore clears the group and world write bits of the build output
once, before the first link is made, under a lock so that no fixture can stage
a helper whose mode has not been settled. The change is one-directional: it
removes write permission for others, keeps the owner's permissions and the
execute bits, and never touches the build output's bytes.

### 6.3 Mutations publish a replacement

`set_mode` and `replace_with_junk` used to `chmod` or overwrite the staged
helper in place. With a shared inode that would reach the Cargo-built helper
and every other fixture, so both now publish a **replacement**: the bytes are
written under a staging name and renamed into place. No fixture mutates a
shared inode.

### 6.4 The server's own staging

`crates/eggwork-server/src/lib.rs` `place_trusted_helper()` stages and then
executes a helper the same way: link-only, with the same mode normalisation and
the same two candidate directories.

## 7. Required tests

| Test | Requirement |
| --- | --- |
| `a_linked_helper_is_executable_immediately_after_publication` | 32 parallel stage-and-spawn cycles of the real helper; zero refusals. Plus a structural assertion that the staged helper is the build output's own inode and device, so a regression to copying fails the test without needing a race to reproduce. |
| `a_group_writable_source_is_normalized_to_a_trusted_mode` | The mode normalisation clears the group and world write bits, keeps the owner's permissions and the execute bits, and is idempotent. |
| the 16 pre-existing Landlock tests | unchanged semantics, unchanged assertions, except that `staging_never_mutates_the_cargo_built_helper` now states the normalisation exception explicitly |

There is deliberately no test for a copy path: the copy path does not exist.

## 8. Required verification

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --locked --workspace
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

Plus a bounded repeated run of the Landlock suite at raised thread count, and
the link-path regression test re-run with `stage` forced back through a copy,
to prove it is non-vacuous.

## 9. Acceptance criteria

1. The refusal is eliminated by construction: the file the fixture executes is
   never written, so the kernel has no reason to refuse it.
2. The elimination is structural — no retry, no sleep, no barrier, no reliance
   on timing or on a host's write-back behaviour.
3. The regression test is non-vacuous, proven by removing the fix.
4. Production behaviour is unchanged; the change is test-only.
5. No new dependency and no `unsafe`.
6. The M004a isolation invariant still holds: no fixture mutates a shared
   inode's content, and the one mode change is one-directional and happens
   before any fixture exists.
7. The M004a record is preserved; the correction is registered as current
   authority in the roadmap and the registry.

## 10. Stop conditions

- If eliminating the refusal requires `unsafe`, a new dependency, a change to
  the trust rules, or weakening a fixture-isolation assertion, stop and record
  the alternative for review instead.
- If the refusal cannot be eliminated but only made rarer, record the residual
  as a finding with its measured rate rather than closing the milestone.

## 11. Closure evidence required

The closure record must distinguish inspected from executed evidence, record
the measurement *and* the kernel source that established the mechanism, show
the regression test failing without the fix, carry the CI evidence that
rejected the copy path, and state the residual risk honestly.
