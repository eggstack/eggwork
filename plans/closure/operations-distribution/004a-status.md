# M004a closure record: qualification helper fixture isolation corrective

Status: **closed** — the fixture-isolation invariant is implemented and proven;
one residual `ETXTBSY` window is characterised and recorded rather than papered over
Closed: 2026-10-05
Plan: `plans/implementation/operations-distribution/004a-release-qualification-fixture-isolation-corrective.md`
Roadmap: `plans/subsystems/operations-distribution-roadmap.md`
Branch: `m004-windows-process-tree`

The corrective itself is complete. A residual intermittent failure was
reproduced, measured, and traced to a host-level kernel window that is neither a
fixture defect nor a production defect. Plan §10's stop condition — "the failure
reproduces after fixture path isolation **and** evidence points to production
helper launch/trust behavior rather than test staging" — is **not** met: after
isolation the failure still reproduces, but the evidence points at neither. That
distinction is the substance of §5 below, and the residual is dispositioned in
§9 rather than left for someone else to rediscover.

## 1. The original failure

Recorded by the Operations M004 closure as one same-head CI failure:

| Item | Value |
| --- | --- |
| Failing test | `required_landlock_allows_workspace_and_denies_outside_reads_and_writes` |
| Head | `33f9a68` |
| Symptom | `Spawn("Text file busy (os error 26)")` from the helper spawn |
| Rerun | passed |
| Attributed cause | "multiple fixtures use a shared helper location derived from the build output/staging path" |

The attributed cause is **partly inaccurate and the correction is still right**.
Inspecting `33f9a68:crates/eggwork-sandbox-helper/tests/landlock_runner.rs` shows
every test already copied the build output into its own `tempfile::tempdir()`
(`.tmpXXXXXX/eggwork-sandbox-helper`). What the tests shared was the *source*
`target/debug/eggwork-sandbox-helper`, which they only ever read. So the
recorded mechanism did not match the code at the failing commit.

That does not make the plan wrong. What the plan asks for is the *invariant* —
build output is input, every fixture owns an immutable executable path, staging
is atomic — and at the recorded baseline none of that was enforced anywhere: it
was true only by convention, duplicated in ten places, and ready to drift. The
corrective makes it structural.

## 2. What changed

`crates/eggwork-sandbox-helper/tests/support/helper_fixture.rs` is now the single
staging path, included by both suites that need a trust-passing helper
(`landlock_runner.rs` and `eggwork-runner`'s `capability_probe.rs`, the latter via
`#[path]` because a dev-only workspace member would be enumerated by the release
contract). Ten duplicated inline blocks became one implementation:

| Plan §4 requirement | Implementation |
| --- | --- |
| 4.1 identify the shared staging path | `built_helper()` is the only reader of the Cargo-built binary; staging is one function |
| 4.2 build output read-only as input | staging never writes the source; asserted by digest **and** mode |
| 4.3 unique per-test installation directory | `tempfile::tempdir()` per fixture, `0700` |
| 4.4 permissions on the copy only | `0755` applied to the staging file before publication |
| 4.5 unique path into `TrustedLandlockSetup` | `TrustedHelperFixture::path()` |
| 4.6 cleanup after convergence | the fixture owns the directory; `run` returns only after the owned tree converges, and removing a directory never disturbs a running process |
| 4.7 no retry/sleep workaround | none added; §5 shows why one would have hidden the real facts |

Staging is atomic: the bytes are written under `….staging` and `rename`d into
place, never written in place. That is not decoration — measured on this host,
against a running executable, `open(O_TRUNC)` and `shutil.copyfile` both return
`ETXTBSY` while `os.replace` succeeds. In-place staging is therefore the one form
that can fail structurally on an executable another test might be running.

## 3. Required tests

All in `crates/eggwork-sandbox-helper/tests/landlock_runner.rs`:

| Plan §6 requirement | Test | What makes it non-vacuous |
| --- | --- | --- |
| two fixtures have distinct executable paths | `two_fixtures_stage_distinct_executable_paths` | also asserts distinct directories and that both staged digests equal the built helper's |
| both may execute/probe concurrently | `concurrent_fixtures_both_execute_concurrently` | two runners, two workspaces, `tokio::join!`, and both receipts must show `SandboxResult::Applied` |
| changing permissions or replacing one cannot affect the other | `mutating_one_fixture_cannot_disturb_another` | breaks one fixture's mode and bytes while the other runs and its digest is re-checked afterwards |
| trust/version negative tests stay meaningful | `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers` (pre-existing, unchanged semantics) plus the junk-helper case in the test above | a broken fixture must lose its advertised capability or fail loudly, never claim isolation |
| required Landlock qualification stays non-vacuous | the whole suite, unchanged assertions | every required-Landlock test still demands `SandboxResult::Applied` or `LimitExceeded` |

No suite-level serialisation was added; nothing in the helper path is a globally
shared resource once each fixture owns its bytes.

## 4. Focused parallel stress evidence

Bounded repetition on this head, Linux, `cargo test --locked -p eggwork-sandbox-helper
--test landlock_runner -- --test-threads=16`:

| Run | Result |
| --- | --- |
| 20 iterations, 15 tests each (300 executions), first pass | 16 green, **4 failed** with `Text file busy (os error 26)` at the helper spawn |
| 20 iterations repeated, host idle | (superseded by §5; the rate is load-dependent) |

The failure rate is load-dependent on this host, and it hits whichever test
happens to spawn a helper at that moment: `malformed_private_spec_and_unavailable_status_channel_do_not_launch_target`
twice, `workspace_symlink_cannot_read_outside_workspace` once, and
`concurrent_fixtures_both_execute_concurrently` once. That distribution is itself
evidence — a fixture-ownership defect would not move between tests at random.

## 5. Root-cause characterisation (measured, not assumed)

A standalone reproducer (16 threads × N iterations, each staging its own private
copy into its own `0700` directory, then spawning it) reproduced the same
`ETXTBSY`, so the suite's own code is not required to trigger it. Facts
established by direct observation:

| Question | Method | Answer |
| --- | --- | --- |
| Is the destination inode shared between fixtures? | per-thread registry of staged paths keyed by inode | No — `same_path_staged_by: 1` on every failure |
| Does something hold the inode open for writing? | `OpenOptions::write(true).open(path)` immediately after the failed spawn | **No** — it succeeds (`write_open: no-writer`) |
| Is it permanent? | three immediate re-spawns | **No** — `["ok", "ok", "ok"]` in almost every failure, occasionally one or two `ETXTBSY` first |
| Is it the copy mechanism? | `fs::copy` vs explicit read+write vs write+`fsync` | All three reproduce it; rates vary with load, not with mechanism |
| Is it a shared *source*? | pre-split private sources vs one shared source | Identical rate (7/480 both) |
| Is it Eggwork's spawn path? | the failing test spawns the helper with plain `std::process::Command`, outside the runner entirely | **No** — it fails the same way |
| Does slowing the process hide it? | `strace -f` on the reproducer | Yes — 0 failures in 1200 iterations |

Reading of those facts: `execve` refused a file that has no writer, on a path
only one thread ever touched, and the window closes within microseconds. The
kernel's `get_write_access`/`deny_write_access` contract (`include/linux/fs.h` on
this host) makes `ETXTBSY` the only exec-time refusal of this shape, and it keys
on the inode's write count — so something transiently held a write-side
reference to the freshly written inode. The precise owner was not identified:
it is gone before any observation is possible, and the window disappears under
`strace`.

What is therefore **established**: the residual failure is a host-level,
load-dependent, microsecond-wide window around executing a just-written
executable, on this machine's ext4, outside Eggwork's code. What is **not**
established: the exact kernel-side holder. That distinction is left visible
rather than papered over with a confident-sounding mechanism.

This also disposes of plan §10: the evidence points away from test staging
(unique paths, no writer, transient) *and* away from production helper
launch/trust behaviour (it reproduces on a plain `std::process::Command` spawn of
a fixture the trust check never sees).

## 6. Ordinary CI evidence

`cargo test --locked --workspace --all-targets` is green, including all 15
Landlock-suite tests and the runner's 16 unit tests. The `rust` CI job is green
on every push recorded in §8; the closure run is `37413709840`, where `rust`,
`release-drift`, and `windows-runner` all succeeded.

Hosted Linux is where the flake was originally recorded and where it still
occurs: the `rust` job runs the same 15 Landlock tests on a GitHub runner, and
those runs have not reproduced it. That asymmetry is itself evidence — the
failure is load-dependent, and a two-core hosted runner is less loaded than the
19-user development host this was characterised on.

## 7. Verification

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | pass, zero warnings |
| `cargo test --locked --workspace --all-targets` | pass |
| `python3 scripts/check_execution_ownership.py` | pass |
| `python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release` | 52 tests, pass |
| `git diff --check` | clean |

## 8. Commits

| Commit | Content |
| --- | --- |
| `0e6682a` | shared atomic per-fixture staging, the five invariant tests, and the Windows-only clippy fixes |
| `098c4a4` | file-level Linux gate for the Landlock suite (found by the new Windows CI job) |
| `edcf056` | gate the Linux-only fixture-path import in the capability probe |

Both suites that stage a trusted helper now go through the one implementation:
`crates/eggwork-sandbox-helper/tests/landlock_runner.rs` via `mod`, and
`crates/eggwork-runner/tests/capability_probe.rs` via
`#[path = "../../eggwork-sandbox-helper/tests/support/helper_fixture.rs"]`. The
`#[path]` include is deliberate and was reviewed as such: the alternative is a
dev-only workspace member, and `release_contract.rs` enumerates workspace
members and their target-scoped dependencies.

## 9. Acceptance criteria

| # | Criterion | Verdict | Evidence |
| --- | --- | --- | --- |
| 1 | No concurrently runnable tests share a mutable helper installation path | **Met** | `two_fixtures_stage_distinct_executable_paths`; `same_path_staged_by: 1` on every observed failure |
| 2 | The Cargo-built helper is never overwritten by a fixture | **Met** | `staging_never_mutates_the_cargo_built_helper` (digest + mode over 8 stagings) |
| 3 | The formerly flaky path passes bounded repeated parallel execution | **Partially met** | 16/20 green; the 4 failures are the residual in §5, characterised and not fixture-caused. Plan §6 asks for a bounded repetition and a recorded count; the count is recorded with the failures rather than omitted |
| 4 | Landlock positive and negative trust tests retain their semantics | **Met** | unchanged assertions; §3 |
| 5 | Production code behaviour is unchanged | **Met** | test-only change; no file under any `src/` except two Windows-lint fixes in `eggworkd.rs`, which are behaviour-preserving `needless_return`/`unused` corrections |
| 6 | M004's open `ETXTBSY` finding can be closed with direct evidence | **Met for the fixture class, not for the kernel window** | §2 and §5 |

Criterion 3 is the one not cleanly met, and the honest reading is: the fixture
invariant is closed, the host-level window is not, and it is not closeable from
inside this repository under plan §8's constraints.

## 10. What was deliberately not done

- **No spawn retry on `ETXTBSY`.** Forbidden by plan §8, and §5 shows it would
  hide the facts rather than fix anything — three immediate retries already
  succeed without one.
- **No sleep.** Forbidden by plan §4.7, for the same reason.
- **No hard links.** A hard link to the build output would eliminate the §5
  window entirely (one inode, never written after cargo finished) — and it
  would also destroy the invariant this milestone exists to establish: `chmod`
  on one fixture's copy changes the mode every other fixture sees, because they
  would all be the same inode. The isolation invariant wins over the flake rate.
- **No trust-rule change.** `verify_trusted_helper` remains owner/mode/ancestor
  trust only. §3's junk-helper test documents what that boundary means instead of
  loosening it: a correctly-permissioned non-helper passes the trust check, and
  the remaining obligation is that execution then fails loudly rather than
  reporting isolation as applied. That test asserts the latter.
- **No suite serialisation** and no CI stress loop, per plan §6.

## 11. Disposition of the Operations M004 finding

M004's unresolved-findings entry for the intermittent `ETXTBSY` is **closed as a
fixture-ownership defect** and **replaced by** a narrower, better-evidenced item:

> Low–medium, test-environment only: on a loaded host, executing a just-written
> executable can transiently fail with `ETXTBSY` (4 of 20 parallel suite runs on
> the closure head). The path is provably unique and unwritten at failure time and
> the window closes immediately, so it is a kernel-side transient rather than a
> harness or product defect. It can reappear in any parallel Linux suite that
> stages and immediately executes a binary. Next step, if it ever needs to be
> eliminated rather than characterised: stage the fixture in a
> `O_TMPFILE`+`linkat` filesystem sequence, which is the only race-free way to
> publish an executable — and it needs `unsafe` or a safe dependency that wraps
> it, both of which this repository currently forbids.

Medium-or-higher: none.

## 12. Reproducing this record

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets
cargo test --locked -p eggwork-sandbox-helper --test landlock_runner -- --test-threads=16
python3 scripts/check_execution_ownership.py
python3 -m unittest discover --start-directory tests/release --top-level-directory tests/release
git diff --check
```

The §5 facts were produced by a throwaway reproducer (16 threads, each staging a
private copy and spawning it) rather than by a committed test, because a test
that needs a loaded host to fail is not a test. Its two decisive measurements —
`same_path_staged_by` and the immediate `write(true).open` probe — are the ones
worth rerunning if this is ever revisited.