# M004b closure record: staged helper write-access corrective

Status: **closed** — the `ETXTBSY` refusal is eliminated by construction, not characterised
Closed: 2026-10-19
Plan: `plans/implementation/operations-distribution/004b-staged-helper-publication-durability-corrective.md`
Roadmap: `plans/subsystems/operations-distribution-roadmap.md`
Branch: `planning/optional-gregg-telemetry-m001-m002`
Corrects: `plans/closure/operations-distribution/004a-status.md` §5, §9 criterion 3, §10, and §11

This record is a corrective addendum. It does not replace the M004a record: that
record is preserved as written, and this one states what in it is no longer
sufficient and what closed the gap.

> **Naming.** The milestone was registered as "staged helper publication
> durability corrective", on the hypothesis that the refusal was a write-back
> problem a durability barrier could close. CI disproved that hypothesis (§3),
> and the executed design is not a barrier at all: the file the fixture executes
> is never written. The milestone ID and the file names are kept so the pointer
> from the M004a record's addendum keeps resolving.

## 1. The failures that triggered this corrective

### 1.1 The original failure

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

### 1.2 The failure that fixed the design

The first attempt at this corrective staged the helper by copying it and closed
the copy with a durability barrier — write, `sync_all`, rename, sync the parent
directory. The barrier measured 0.02% on the development host and was carried to
CI, where it did not hold:

| Item | Value |
| --- | --- |
| CI runs | `39316444280` (push), `39316541191` (pull request) |
| Head | `0accbdc` (`docs(plans): close Operations M004b …`) |
| Job | `rust` |
| Failing test | `a_staged_helper_is_executable_immediately_after_publication` — the new *copy-path* regression test |
| Symptom | `a staged helper was refused: Text file busy (os error 26)`, three times in 32 stagings |
| Assertion | `left: 3, right: 0` — "a freshly published helper must be executable without an ETXTBSY refusal" |
| Suite result | `test result: FAILED. 16 passed; 1 failed` — job exit code 101 |

The same run's *link-path* test — 32 parallel stagings of the same helper,
staged by hard link — passed 32 of 32.

That contrast is the finding. A barrier that is 0.02% on one host and 9% on
another is not a barrier; it is a rate. The copy path was removed, and the
staged helper is now published without ever being written.

## 2. The historical claims that are no longer sufficient

The M004a closure record §5 concluded that the residual failure was "a
host-level, load-dependent, microsecond-wide window around executing a
just-written executable, on this machine's ext4, outside Eggwork's code", and
§9 criterion 3 was recorded as only *partially met* on that basis. §10 left
the residual open by rejecting the obvious fixes, and §11 pointed at
`O_TMPFILE`+`linkat` publication as the next step "if it ever needs to be
eliminated rather than characterised", which the repository's rules forbid.

That characterisation was accurate about what was observed and wrong about what
it implied. Five specific claims do not survive the evidence below:

1. **"Microsecond-wide window" implies a timing race.** It is not. The
   condition is a property of the inode and it persists: 100 immediate retries
   of the same path all failed (§5.1).
2. **"A file that has no writer" was inferred from a probe that could not have
   detected a writer.** The M004a §5 probe was
   `OpenOptions::write(true).open(path)` run *after* the failed spawn. Opening
   a file for writing succeeds whether or not another writer exists, so that
   probe could not distinguish the two cases. The kernel's own accounting says
   why: `get_write_access()` refuses only when the count is *negative*, so a
   file that is open for writing can still be opened for writing again.
3. **"Atomic publication therefore cannot hit `ETXTBSY`"** — the fixture
   module's own property 2 — was false, and that false claim is what left CI
   red. The M004a record itself had already recorded that atomic publication
   was not enough (§5: "All three reproduce it; rates vary with load, not with
   mechanism"), but the module header kept asserting the opposite.
4. **§10's rejection of hard links was reasoned from a fixture design that
   mutated a shared inode.** It is superseded here, for the reason given in §6.
5. **The first attempt's own hypothesis — that the refusal is a write-back
   window that a durability barrier closes — is wrong.** The barrier was
   carried to CI and refused 3 times in 32 stagings there (§1.2). The
   mechanism is the inode's write *access*, not its write-back state.

## 3. What changed

### 3.1 The staged helper is a link, and there is no copy fallback

`crates/eggwork-sandbox-helper/tests/support/helper_fixture.rs` — the single
staging path shared by the Landlock suite and the runner's capability probe —
now **links** the Cargo-built helper into the fixture's private `0700`
directory:

```text
stage(source):
    source mode normalised to a trusted mode, once, before the first link
    fixture directory, 0700
    hard link source -> <dir>/eggwork-sandbox-helper      # writes nothing
    on failure (different filesystem, or a source that cannot be linked):
        fail loudly
```

A link shares the source's inode and never opens it for writing, so the
inode's write access is never taken and the kernel has no reason to refuse to
execute it. That is the difference between a refusal that cannot happen and one
that is merely unlikely. It also removes a per-fixture copy of the whole binary
from every test.

**There is no copy fallback.** A copy is by definition a file that was written,
and a written file is exactly the construction the CI evidence refuses. The
first attempt's fallback — copy, commit, publish — is what CI rejected, and
keeping it "for the cases where linking does not work" would have kept the
flake alive on exactly the hosts that cannot afford it. When a link is
impossible, staging fails loudly and names the reason.

The system temporary directory is tried first, because its ancestor chain is the
one `verify_trusted_helper` accepts by construction (`/tmp` is a sticky root
directory, which the trust check allows explicitly). A directory next to the
source is the fallback, because it is the same filesystem by definition.

### 3.2 A link carries the source's mode, so the source is normalised once

`verify_trusted_helper` rejects a helper that anyone but its owner may write,
and a link shares the source's mode. Cargo derives the executable's mode from
the process umask — `0666 & ~umask` plus the execute bits — so a permissive
umask (0002 is common in shared development environments; the development host
for this record uses it) leaves the build output group-writable, and no fixture
could stage it by linking.

The fixture therefore clears the group and world write bits of the build output
once, before the first link is made, under a lock so that no fixture can stage
a helper whose mode has not been settled yet. The change is deliberately
one-directional:

- it removes write permission for everyone else;
- it keeps the owner's permissions, the execute bits, and every other bit;
- it never touches the build output's bytes;
- it can only ever turn an untrusted helper into a trusted one, never the
  reverse.

The M004a invariant — "a fixture must never mutate, truncate, overwrite, or
rename over an executable path another concurrently runnable test may execute" —
is not weakened by it: the build output's *content* is untouched, and the mode
change happens once, before any fixture exists. The M004a proof
`staging_never_mutates_the_cargo_built_helper` now states the exception
explicitly and asserts that no *further* staging changes the mode.

### 3.3 Mutations publish a replacement instead of changing the inode

`set_mode` and `replace_with_junk` used to `chmod` or overwrite the staged
helper in place. With a shared inode that would reach the Cargo-built helper
and every other fixture, so both now publish a **replacement**: the bytes are
written under a staging name and renamed into place. No fixture mutates a
shared inode any more, which is a strictly stronger form of the M004a isolation
invariant than the one it recorded.

### 3.4 The server's own staging

`crates/eggwork-server/src/lib.rs` `place_trusted_helper()` stages and then
executes a helper the same way. It now links, normalises the build output's
mode the same way, and uses the same two candidate directories.

### 3.5 What was deliberately not done

- **No retry on `ETXTBSY`, and no sleep.** Both are forbidden by the M004a
  plan, and §5.1 shows a retry does not work: the condition is per-inode and
  survived 100 immediate retries. A retry would have converted a deterministic
  refusal into an intermittent one while still leaving the suite red.
- **No durability barrier.** The first attempt carried one, and CI refused it
  3 times in 32 stagings (§1.2). The barrier addressed write-back, which is not
  the property.
- **No `O_TMPFILE`+`linkat`.** §11's suggested next step. It needs `unsafe` or
  a new dependency, and it addresses publication atomicity, not write access.
- **No trust-rule change.** `verify_trusted_helper` remains owner/mode/ancestor
  trust only. It accepts a hard link exactly as it accepts a copy: the trust
  rules are about a path's metadata, not about the inode behind it being
  private.
- **No change to production code.** The product never stages a helper. It
  consumes one that was installed long before, so its inode carries no write
  access. The exposure was confined to test fixtures.

## 4. The mechanism, read from the kernel rather than inferred

The first attempt described the refusal as "a file whose inode still carries a
write reference", and stated in its own residual-risk section that the
mechanism was "described from observation, not from kernel source". It is now
read from the source. The relevant accounting is `struct inode::i_writecount`,
whose documented meaning is in `include/linux/fs.h`:

> The i_writecount field of an inode can have the following values:
> 0: no write access, no denied write access
> < 0: (-i_writecount) users that denied write access to the file.
> > 0: (i_writecount) users that have write access to the file.

Four call sites close the loop.

1. **The write access is taken by every write-open of a regular file.**
   `fs/open.c`, `do_dentry_open()`:

   ```c
   } else if (f->f_mode & FMODE_WRITE && !special_file(inode->i_mode)) {
           error = file_get_write_access(f);
           if (unlikely(error))
                   goto cleanup_file;
           f->f_mode |= FMODE_WRITER;
   }
   ```

   `file_get_write_access()` calls `get_write_access()`, which is
   `atomic_inc_unless_negative(&inode->i_writecount)`.

2. **The refusal is raised on the execution side.** `fs/exec.c`,
   `do_open_execat()` — the function every `execve()` goes through — ends with:

   ```c
   err = exe_file_deny_write_access(file);
   if (err)
           return ERR_PTR(err);
   ```

   and `exe_file_deny_write_access()` is `deny_write_access()`:

   ```c
   static inline int deny_write_access(struct file *file)
   {
           struct inode *inode = file_inode(file);
           return atomic_dec_unless_positive(&inode->i_writecount) ? 0 : -ETXTBSY;
   }
   ```

   **The decrement is refused exactly when the count is positive — that is,
   when the inode is open for writing — and the refusal is `-ETXTBSY`.**

3. **The write access is released with the file.** The release is paired with
   the teardown of the file that holds it (`FMODE_WRITER` →
   `put_write_access(file_inode(file))`), and the teardown is *deferred*:
   `fs/file_table.c`, `fput()` → `__fput_deferred()`, which queues the work as
   a task work, and falls back to a workqueue when the task work cannot be
   used. The release is therefore not synchronous with the `close()` that
   dropped the last reference, and under concurrency it can lag the next
   `execve()`.

4. **The release is not a write-back event.** Nothing in that path consults
   the page cache or the dirty state, which is why `fsync`, `O_DSYNC` and
   sleeping do not help (§5.2) and why the first attempt's barrier did not
   hold on CI.

Two consequences decide the design:

- **Only a file that is never opened for writing can be executed without
  this refusal.** A copy is opened for writing by definition. A link is not.
- **The refusal is per-inode and survives immediate retries**, because the
  write access is still held: a refused `execve()` does not release it.

## 5. The measurements

### 5.1 The condition is per-inode and persistent, not a timing window

| Question | Method | Answer |
| --- | --- | --- |
| Does it need Eggwork's code? | standalone `std` reproducer, 16 threads | **No** — 15 failures in 190 attempts |
| Does anything hold the failing inode open? | match the inode number against every `/proc/<pid>/fd`, reading each holder's `fdinfo` flags | **No** — no holder at any pid, which is consistent with §4.3: the access outlives the descriptor |
| Can the inode be reopened for writing? | `OpenOptions::write(true).open(path)` at failure time | **Yes** — so the M004a probe was uninformative |
| Is it transient? | 100 immediate re-spawns of the same path | **No** — every retry failed |
| Is it `copy_file_range`-specific? | `fs::copy` vs explicit read+write | **No** — 31.00% vs 37.08% |
| Is it the rename? | publish by rename vs exec the staging name | Rename helps, but 44.06% without it |
| Is it the *write* of the file? | hard link to the source, never written | **No** — 0.00% |
| Is it Eggwork's spawn path? | the failing test spawns with plain `std::process::Command` | **No** |

### 5.2 The kernel's own accounting, confirmed by a direct probe

A standalone `std` probe on the development host, using the real
Cargo-built helper as the payload so that the only variable is the staging
sequence:

| Staging sequence | `execve` of the result |
| --- | --- |
| write, close | refused under concurrency (§5.3) |
| write, close, `fsync(file)` | refused under concurrency — the barrier does not help |
| write, close, `O_DSYNC` writes | refused under concurrency — write-through does not help |
| write, close, sleep 2 s | refused under concurrency — waiting does not help |
| write, close, **hold a write descriptor open** | **`ETXTBSY`, every time, single-threaded** |
| write, close, hold a write descriptor open, then drop it | accepted — the access is released with the teardown |
| open for writing, close, exec | accepted |
| hard link to the source, exec | accepted |
| exec the Cargo-built helper directly | accepted |

The only construction that reproduces the refusal on demand is the one the
kernel documents: an inode that is open for writing.

### 5.3 Variant measurement

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

Read against §4, these numbers say something different from what the first
attempt read into them. The commit-based variants are not "the cure"; they are
variants that happen to leave more wall-clock time between the `close()` and
the `execve()`, and on a slow disk that is enough for the deferred teardown to
run. The development host's disk is slow (an 8 MB copy costs ~170 ms, and the
`fsync` variants ~600 ms), which is why its "committed" variants measured 0.00%
and why the CI runner — whose disk is fast, so the window between the `close()`
and the `execve()` is short — still refused 3 stagings in 32 *with* the barrier
in place (§1.2).

The variant that is zero on every host, every load, and every filesystem is
the one that never opens the executed inode for writing.

### 5.4 A note on the M004a §11 pointer

§11's suggested next step, `O_TMPFILE`+`linkat`, addresses publication
atomicity. The failure was never that a partially written helper could be
observed — the M004a staging already renamed atomically. It was that a fully
written helper was still open for writing. `O_TMPFILE` does not change that,
because the inode it publishes is still open for writing when it is linked.

## 6. Why hard links, against M004a §10

M004a §10 rejected hard links with a specific reason:

> A hard link to the build output would eliminate the §5 window entirely (one
> inode, never written after cargo finished) — and it would also destroy the
> invariant this milestone exists to establish: `chmod` on one fixture's copy
> changes the mode every other fixture sees, because they would all be the
> same inode. The isolation invariant wins over the flake rate.

The rejection is sound on its facts and no longer decides the case:

1. **Its premise is confirmed.** The §5 measurement is right: a hard link
   eliminates the refusal entirely, 0 refusals in 1 920 attempts, and 0 of 32
   on CI while the copy path was refused 3 times in the same run.
2. **Its stated reason is answered by this corrective.** The reason was that
   `chmod` on a shared inode reaches every fixture. §3.3 removes the `chmod`: a
   mode change now publishes a replacement, so no fixture mutates a shared
   inode. The isolation invariant is preserved — and strengthened, because the
   Cargo-built helper is now provably never mutated by any fixture.
3. **Its trade-off has been overtaken by events.** M004a weighed the isolation
   invariant against the flake rate. The flake rate is now what makes CI red:
   `39306109740` failed on it, and `39316541191` failed on the copy path that
   M004b first proposed. M004a's own §11 asked what to do "if it ever needs to
   be eliminated rather than characterised", and this is that moment.

The one cost M004a did not have to weigh is §3.2's mode normalisation, because
M004a never linked. It is recorded as a residual risk in §9 rather than hidden.

The M004a record is preserved as written, with additive forward pointers
appended. Nothing in its findings, evidence, or verdicts is changed.

## 7. Required tests

`crates/eggwork-sandbox-helper/tests/landlock_runner.rs`:

| Test | What makes it non-vacuous |
| --- | --- |
| `a_linked_helper_is_executable_immediately_after_publication` | 32 threads each stage the real helper and spawn it; no spawn may be refused. **and** the staged helper is asserted to be the build output's own inode and device, so a regression to copying fails the test outright, with no load, no timing and no lucky window needed. |
| `a_group_writable_source_is_normalized_to_a_trusted_mode` | The normalisation clears the group and world write bits, keeps the owner's permissions and the execute bits, and is idempotent. |

Both halves of the first test were verified by removing the fix, not asserted.
With `stage` forced back through a copy on the development host:

| Check | Result |
| --- | --- |
| the refusal count | **5 of 32** stagings refused with `Text file busy` — the test fails on the count |
| the refusal count, second run | **9 of 32** refused |
| the inode assertion, with the count assertion disabled | fails deterministically: `left: 3038061, right: 17962722` — the staged helper is not the build output's inode |

The second row is the one that matters most: the structural half of the test
does not need the race to reproduce at all, so a regression to copying is
caught on the first run rather than on the run where the host happens to be
busy.

The copy-path regression test that the first attempt added
(`a_staged_helper_is_executable_immediately_after_publication`) was **removed**
with the copy path it covered. It was not a weak test — it did its job, and it
is what proved on CI that a committed copy is still refusable. Asserting that
a *written* file is executable is asserting something the kernel does not
guarantee, and no amount of tuning makes it true.

The pre-existing suite keeps its semantics: 16 Landlock tests, including the
five M004a isolation proofs, all still pass unchanged. No assertion was
weakened, and no suite-level serialisation was added.

## 8. Verification

| Command | Result |
| --- | --- |
| `cargo test --locked -p eggwork-sandbox-helper --test landlock_runner` | pass — 17 passed, 0 failed |
| the same suite, repeated runs at `--test-threads=16` | pass — 0 `ETXTBSY`; see §8.1 |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | pass, zero warnings |
| `cargo test --locked --workspace --all-targets` | pass — 251 tests across 8 suites, 1 pre-existing ignored |
| `cargo check --locked --workspace` | pass |
| `python3 scripts/check_execution_ownership.py` | pass |
| `python3 scripts/check_execution_ownership.py --prove-negative-exit` | pass |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | 52 tests, pass |
| `git diff --check` | clean |

### 8.1 The repeated-run evidence

The link path publishes a directory entry and writes nothing, so the suite no
longer has a staging race to lose. The repeated runs are recorded because the
first attempt's failure was a *rate* rather than a certainty: the rate has to
be measured on the host that failed, not on the host that is comfortable.

| Host | Construction | Attempts | Refusals |
| --- | --- | --- | --- |
| development host, 16 threads | copy, rename | 1 920 | 687 (35.78%) |
| development host, 16 threads | copy, rename, commit the directory | 1 920 | 0 |
| development host, 16 threads | copy, no barrier (the forced regression) | 32 | **5** |
| development host, 16 threads | hard link | 40 suite runs, 32 stagings each | **0** |
| CI runner | copy, rename, commit the file, commit the directory | 32 | **3 (9.38%)** |
| CI runner | hard link | 32 | **0** |

The CI row for the copy is the finding that closed the copy path; the CI row
for the link is the one that kept it.

## 9. Residual risk

- **The staged helper's mode comes from the build output.** A link cannot have
  a different mode from the file it links to, so the fixture normalises the
  build output's mode once (§3.2). That is the one place where a fixture
  touches the shared build artifact, and it is a mode change that can only
  make the artifact more trusted. Its bytes are never touched.
- **The fallback directory is not trusted by construction.** When the source
  and the system temporary directory are on different filesystems, the fixture
  stages into a directory next to the source, whose ancestor chain is the
  build tree's. On a checkout under a group- or world-writable directory, the
  trust check would reject that helper — loudly, as
  "sandbox helper ancestor trust check failed", not silently. The system
  temporary directory is preferred precisely because its chain is trusted.
- **A published replacement is a file that was written.** `set_mode` and
  `replace_with_junk` publish written files, so executing one may be refused
  with `ETXTBSY` instead of failing as a malformed executable. Both are loud
  failures, and the junk fixture is never expected to run successfully — the
  M004a proof asserts exactly that it fails loudly rather than reporting
  isolation as applied.
- **The measured zero is a zero on the measured hosts and workloads.** 0
  refusals in 1 920 attempts at 16 threads on the development host, and 0 of 32
  on the CI runner, against a 22.6% rate for the construction it replaced.
  The structural argument in §4 is what carries the guarantee; the
  measurements are what confirm it.
- **Medium-or-higher findings: none.**

## 10. Registry and roadmap disposition

- `plans/subsystems/operations-distribution-roadmap.md` carries milestone
  **M004b — staged helper write-access corrective**, status **closed**, pointing
  at this record.
- `plans/registry.md` carries a closure row for M004b and an updated
  Operations/distribution workstream row, so that the current authority is the
  corrected state rather than M004a's characterisation.
- `plans/closure/operations-distribution/004a-status.md` is **preserved as
  written**. Two additive forward pointers are appended to it: one recording
  that the residual it characterised was subsequently eliminated and that its
  §10 hard-link rejection is superseded (§6), and one recording that the
  elimination is by link-only staging rather than by the copy-and-commit
  barrier the first attempt proposed. Nothing in the M004a record's findings,
  evidence, or verdicts is changed.

## 11. Reproducing this record

```text
cargo test --locked -p eggwork-sandbox-helper --test landlock_runner
cargo test --locked -p eggwork-sandbox-helper --test landlock_runner -- --test-threads=16
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo check --locked --workspace
python3 scripts/check_execution_ownership.py
python3 scripts/check_execution_ownership.py --prove-negative-exit
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

The §5 measurements were produced by throwaway reproducers (a standalone
`std` program, a multi-variant harness and a kernel-source read), not by
committed tests: a measurement that needs a loaded host to fail is a
measurement, not a test. The two committed tests are the ones in §7, and both
fail loudly when the corresponding fix is removed.
