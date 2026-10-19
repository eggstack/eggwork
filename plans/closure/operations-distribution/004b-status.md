# M004b closure record: staged helper publication durability corrective

Status: **closed** — the residual `ETXTBSY` refusal is eliminated by construction, not characterised
Closed: 2026-10-19
Plan: `plans/implementation/operations-distribution/004b-staged-helper-publication-durability-corrective.md`
Roadmap: `plans/subsystems/operations-distribution-roadmap.md`
Branch: `planning/optional-gregg-telemetry-m001-m002`
Corrects: `plans/closure/operations-distribution/004a-status.md` §5, §9 criterion 3, §10, and §11

This record is a corrective addendum. It does not replace the M004a record: that
record is preserved as written, and this one states what in it is no longer
sufficient and what closed the gap.

## 1. The failure that triggered this corrective

Recorded by the CI run on the optional-Gregg host-telemetry branch, and the
first CI failure of this shape to land on a red build:

| Item | Value |
| --- | --- |
| CI run | `39306109740` |
| Head | `56880ee` (`docs(plans): close optional Gregg host telemetry M002`) |
| Branch | `planning/optional-gregg-telemetry-m001-m002` |
| Job | `rust` — `cargo test --locked --workspace --all-targets` |
| Failing test | `malformed_private_spec_and_unavailable_status_channel_do_not_launch_target` |
| Panic site | `crates/eggwork-sandbox-helper/tests/landlock_runner.rs:471` |
| Symptom | `called Result::unwrap() on an Err value: Os { code: 26, kind: ExecutableFileBusy, message: "Text file busy" }` |
| Suite result | `test result: FAILED. 14 passed; 1 failed` — job exit code 101 |

The failing test is not the product's code path. It is a test that stages a
trusted helper into a private directory and then spawns it with plain
`std::process::Command`, outside the runner entirely.

## 2. The historical claim that is no longer sufficient

The M004a closure record §5 concluded that the residual failure was "a
host-level, load-dependent, microsecond-wide window around executing a
just-written executable, on this machine's ext4, outside Eggwork's code", and
§9 criterion 3 was recorded as only *partially met* on that basis. §10 left
the residual open by rejecting the obvious fixes, and §11 pointed at
`O_TMPFILE`+`linkat` publication as the next step "if it ever needs to be
eliminated rather than characterised", which the repository's rules forbid.

That characterisation was accurate about what was observed and wrong about what
it implied. Four specific claims do not survive the evidence below:

1. **"Microsecond-wide window" implies a timing race.** It is not. The
   condition is a property of the inode and it persists: 100 immediate retries
   of the same path all failed (§4). A window that a hundred back-to-back
   attempts cannot clear is not a timing window.
2. **"A file that has no writer" was inferred from a probe that could not have
   detected a writer.** The M004a §5 probe was
   `OpenOptions::write(true).open(path)` run *after* the failed spawn. Opening
   a file for writing succeeds whether or not another writer exists, so that
   probe could not distinguish the two cases and could not have identified a
   holder. The scan actually used here — matching the failing inode number
   against every `/proc/<pid>/fd` entry and reading each holder's
   `/proc/<pid>/fdinfo/<fd>` flags — finds no holder at all.
3. **"Atomic publication therefore cannot hit `ETXTBSY`"** — the fixture
   module's own property 2 — was false, and that false claim is what left CI
   red. The M004a record itself had already recorded that atomic publication
   was not enough (§5: "All three reproduce it; rates vary with load, not with
   mechanism"), but the module header kept asserting the opposite.
4. **§10's rejection of hard links was reasoned from a fixture design that
   mutated a shared inode.** It is superseded here, for the reason given in
   §5.2.

## 3. What changed

### 3.1 The staged helper is no longer written

`crates/eggwork-sandbox-helper/tests/support/helper_fixture.rs` — the single
staging path shared by the Landlock suite and the runner's capability probe —
now **links** the Cargo-built helper into the fixture's private `0700`
directory instead of copying it:

```text
stage(source):
    fixture directory, 0700
    hard link source -> <dir>/eggwork-sandbox-helper      # writes nothing
    on failure (different filesystem, or a source that is not executable):
        copy -> chmod 0755 -> commit(file) -> rename -> commit(directory)
```

A link shares the source's inode, and that inode was last written when the
linker produced the binary — long before any test ran. It therefore carries no
write reference, so the kernel has no reason to refuse to execute it. That is
the difference between a refusal that cannot happen and one that is merely
unlikely. It also removes a per-fixture copy of the whole binary from every
test.

The fallback — a private copy — is committed before it is published, using the
repository's own durability ordering (write, `sync_all`, `rename`, sync the
parent directory), the same one the blob store and the drain marker use (see
`architecture/data-plane.md` and `architecture/operations-cli.md`).

### 3.2 Mutations publish a replacement instead of changing the inode

`set_mode` and `replace_with_junk` used to `chmod` or overwrite the staged
helper in place. With a shared inode that would reach the Cargo-built helper
and every other fixture, so both now publish a **replacement**: the bytes are
written under a staging name, committed, and renamed into place. No fixture
mutates a shared inode any more, which is a strictly stronger form of the M004a
isolation invariant than the one it recorded.

### 3.3 The server's own staging

`crates/eggwork-server/src/lib.rs` `place_trusted_helper()` stages and then
executes a helper the same way. It now links when it can and commits a copy
when it cannot, for the same reasons.

### 3.4 What was deliberately not done

- **No retry on `ETXTBSY`, and no sleep.** Both are forbidden by the M004a
  plan, and §4 shows a retry does not work: the condition is per-inode and
  survived 100 immediate retries. A retry would have converted a deterministic
  refusal into an intermittent one while still leaving the suite red.
- **No `O_TMPFILE`+`linkat`.** §11's suggested next step. It needs `unsafe` or
  a new dependency, and the evidence says it addresses the wrong property: the
  failure is not that a partially written file can be observed, it is that a
  fully written file still carries a write reference.
- **No trust-rule change.** `verify_trusted_helper` remains owner/mode/ancestor
  trust only, and it accepts a hard link exactly as it accepts a copy: the
  trust rules are about a path's metadata, not about the inode behind it being
  private.
- **No change to production code.** The product never stages a helper. It
  consumes one that was installed long before, so its inode carries no write
  reference. The exposure was confined to test fixtures.

## 4. Root cause, measured rather than assumed

The investigation used a standalone `std`-only reproducer (no Eggwork code)
that stages a private copy of an executable and spawns it, so that a
reproduction could not depend on the workspace's own fixture code.

### 4.1 The condition is per-inode and persistent, not a timing window

| Question | Method | Answer |
| --- | --- | --- |
| Does it need Eggwork's code? | standalone `std` reproducer, 16 threads | **No** — 15 failures in 190 attempts |
| Does anything hold the failing inode open? | match the inode number against every `/proc/<pid>/fd`, reading each holder's `fdinfo` flags | **No** — no holder at any pid |
| Can the inode be reopened for writing? | `OpenOptions::write(true).open(path)` at failure time | **Yes** — so the M004a probe was uninformative |
| Is it transient? | 100 immediate re-spawns of the same path | **No** — every retry failed |
| Is it `copy_file_range`-specific? | `fs::copy` vs explicit read+write | **No** — 31.00% vs 37.08% |
| Is it the rename? | publish by rename vs exec the staging name | Rename helps, but 44.06% without it |
| Is it the *write* of the file? | hard link to the source, never written | **No** — 0.00% |
| Is it Eggwork's spawn path? | the failing test spawns with plain `std::process::Command` | **No** |

### 4.2 Variant measurement

Each variant stages a private copy and spawns it, 16 threads in parallel. The
staged copy always lands in `/tmp`; only the source differs between runs.

With the 8 MB Cargo-built helper as the source, 1 920 attempts per variant:

| Variant | `ETXTBSY` | Rate |
| --- | --- | --- |
| copy, rename (the M004a sequence) | 687 | **35.78%** |
| write, commit the file, rename | 15 | 0.78% |
| copy, rename, commit the directory | 0 | 0.00% |
| copy, no rename | 846 | 44.06% |
| hard link to the source, never written | 0 | **0.00%** |
| execute the source directly | 0 | **0.00%** |
| copy, rename, retry up to 100 times | 4 | 0.21% (all 4 hit the retry cap) |

With the 8 MB helper again, 4 800 attempts per variant, which is what
separated the two barriers:

| Variant | `ETXTBSY` | Rate |
| --- | --- | --- |
| copy, rename | 1 488 | **31.00%** |
| read + write, no commit | 1 780 | **37.08%** |
| copy, rename, commit the directory | 1 | 0.02% |

With a 27 KB source, 32 000 attempts per variant, which is what separated the
orderings:

| Variant | `ETXTBSY` | Rate |
| --- | --- | --- |
| copy, rename | 7 219 | **22.56%** |
| copy, rename, commit the directory | 0 | 0.00% |
| copy, commit the file, rename | 1 | 0.003% |
| copy, rename, commit the file, commit the directory | 0 | 0.00% |
| copy, commit the data, rename, commit the directory | 0 | 0.00% |
| copy, rename, commit the directory, commit the file | 1 | 0.003% |
| copy, commit the file, rename, commit the directory | 0 | 0.00% |

Three readings matter. The trigger is **writing the file at all**: a file whose
inode was never written after its last write-back is executable every time,
which is why the link path removes the failure rather than reducing it. The
cure is **completing the write-back before executing**, not avoiding the
write. And the barrier has to come *after* the mode change, because the mode
change is itself a write to the inode — a barrier taken before it does not
cover it, which is why the 8 MB "commit the file, rename" variant still
refused 15 times out of 1 920 while the ordering that commits after the mode
change refused none.

### 4.3 A note on the M004a §11 pointer

§11's suggested next step, `O_TMPFILE`+`linkat`, addresses publication
atomicity. The failure was never that a partially written helper could be
observed — the M004a staging already renamed atomically. It was that a fully
written helper still carried a write reference. `O_TMPFILE` does not change
that, because the inode it publishes is still open for writing when it is
linked.

## 5. Why hard links, against M004a §10

M004a §10 rejected hard links with a specific reason:

> A hard link to the build output would eliminate the §5 window entirely (one
> inode, never written after cargo finished) — and it would also destroy the
> invariant this milestone exists to establish: `chmod` on one fixture's copy
> changes the mode every other fixture sees, because they would all be the
> same inode. The isolation invariant wins over the flake rate.

The rejection is sound on its facts and no longer decides the case:

1. **Its premise is confirmed.** The §5 measurement is right: a hard link
   eliminates the window entirely, 0 refusals in 1 920 attempts.
2. **Its stated reason is answered by this corrective.** The reason was that
   `chmod` on a shared inode reaches every fixture. §3.2 removes the `chmod`:
   a mode change now publishes a replacement, so no fixture mutates a shared
   inode. The isolation invariant is preserved — and strengthened, because the
   Cargo-built helper is now provably never mutated by any fixture.
3. **Its trade-off has been overtaken by events.** M004a weighed the isolation
   invariant against the flake rate. The flake rate is now what makes CI red:
   `39306109740` failed on it. M004a's own §11 asked what to do "if it ever
   needs to be eliminated rather than characterised", and this is that moment.

The M004a record is preserved as written, with one additive forward pointer
appended. Nothing in its findings, evidence, or verdicts is changed.

## 6. Required tests

`crates/eggwork-sandbox-helper/tests/landlock_runner.rs` gains two tests:

| Test | What makes it non-vacuous |
| --- | --- |
| `a_linked_helper_is_executable_immediately_after_publication` | 32 threads each stage the real helper and spawn it; no spawn may be refused. **Verified to fail when `stage` is forced back through an unbarriered copy**: 6 of the 32 stagings were refused with `Text file busy (os error 26)` and the test failed. This is the direct regression test for the CI failure, and it costs nothing, because staging a link writes nothing. |
| `a_staged_helper_is_executable_immediately_after_publication` | 32 threads each publish a private copy and spawn it; no spawn may be refused. **Verified to fail when the copy path's barrier is removed**: 2 of the 32 were refused and the test failed. |

The pre-existing suite keeps its semantics: 16 Landlock tests, including the
five M004a isolation proofs, all still pass unchanged. No assertion was
weakened, and no suite-level serialisation was added.

## 7. Verification

| Command | Result |
| --- | --- |
| `cargo test --locked -p eggwork-sandbox-helper --test landlock_runner` | pass — 17 passed, 0 failed |
| the same suite, 40 consecutive runs at `--test-threads=16` | pass — 0 `ETXTBSY` in 40 runs (5 of 25 runs failed the same way before the fix) |
| link-path regression test with `stage` forced through an unbarriered copy | **fails as intended** — 6 of 32 stagings refused |
| copy-path regression test with the barrier removed | **fails as intended** — 2 of 32 stagings refused |
| suite duration | 3.6 s, unchanged from before the fix — the per-fixture 8 MB copy is gone |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | pass, zero warnings |
| `cargo test --locked --workspace --all-targets` | pass |
| `cargo check --locked --workspace` | pass |
| `python3 scripts/check_execution_ownership.py` | pass |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | 52 tests, pass |
| `git diff --check` | clean |

## 8. Acceptance criteria

| # | Criterion | Verdict | Evidence |
| --- | --- | --- | --- |
| 1 | The `ETXTBSY` failure is eliminated, not characterised | **Met** | §4.2: a file that is never written is executable every time; 40 consecutive suite runs green |
| 2 | The elimination is structural, not a retry or a sleep | **Met** | §3.4; §4.1 shows a retry does not clear the condition |
| 3 | The correction is proven non-vacuous | **Met** | §6: both regression tests fail when the corresponding fix is removed |
| 4 | Production behaviour is unchanged | **Met** | test-only change; the only file under a `src/` touched is `eggwork-server/src/lib.rs`, inside its `#[cfg(test)]` module |
| 5 | No new dependency, no `unsafe`, no new platform surface | **Met** | pure `std`; `unsafe_code = "deny"` untouched |
| 6 | The M004a isolation invariant still holds | **Met** | §3.2: no fixture mutates a shared inode; the five M004a proofs pass unchanged |
| 7 | The M004a record is preserved, not rewritten | **Met** | §9 |
| 8 | The registry and the roadmap state the new authority | **Met** | §9 |

## 9. Registry and roadmap disposition

- `plans/subsystems/operations-distribution-roadmap.md` gains milestone
  **M004b — staged helper publication durability corrective**, status
  **closed**, pointing at this record.
- `plans/registry.md` gains a closure row for M004b and updates the
  Operations/distribution workstream row, so that the current authority is the
  corrected state rather than M004a's characterisation.
- `plans/closure/operations-distribution/004a-status.md` is **preserved as
  written**. One additive forward pointer is appended to it, marking that the
  residual it characterised was subsequently eliminated, that its §10
  hard-link rejection is superseded for the reason given in §5, and where the
  elimination is recorded. Nothing in the M004a record's findings, evidence,
  or verdicts is changed.

## 10. Residual risk

- **The link path depends on the source being executable and not writable by
  anyone else**, and on the source sharing a filesystem with the fixture
  directory. When either does not hold, the fixture falls back to a committed
  private copy. The fallback is exercised by
  `a_staged_helper_is_executable_immediately_after_publication`, so it is not
  an untested path.
- **The copy path's completeness was measured at 0.02% for a
  directory-only commit with the 8 MB helper**, which is why the file is
  committed as well and why the commit follows the mode change. That path is
  now used only for mutations — whose fixtures are never expected to execute
  successfully — and for the fallback.
- **The measured zero is a zero on this host and this workload.** The
  measurement is 0 refusals in 32 000 attempts at 16 threads, which is the
  shape that produced a 22.6% rate before the fix.
- **The mechanism is described from observation, not from kernel source.** The
  claim "the write reference is released when the write-back completes" is the
  reading that fits every measurement in §4, including the counter-intuitive
  one that a file nobody holds open can still be refused. It was not confirmed
  against the kernel's own `i_writecount` accounting on this host, so the
  record states it as the explanation that fits the evidence rather than as a
  quoted kernel invariant.
- **Medium-or-higher findings: none.**

## 11. Reproducing this record

```text
cargo test --locked -p eggwork-sandbox-helper --test landlock_runner
cargo test --locked -p eggwork-sandbox-helper --test landlock_runner -- --test-threads=16
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --locked --workspace
python3 scripts/check_execution_ownership.py
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

The §4 measurements were produced by throwaway reproducers (a standalone
`std` program and a multi-variant harness), not by committed tests: a
measurement that needs a loaded host to fail is a measurement, not a test. The
two committed tests are the ones in §6, and both fail loudly when the
corresponding fix is removed.
