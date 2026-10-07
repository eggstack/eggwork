# eggwork-sandbox-helper

`eggwork-sandbox-helper` is a standalone Linux binary — `crates/eggwork-sandbox-helper/src/main.rs` (401 lines, `#![forbid(unsafe_code)]`) plus `tests/landlock_runner.rs` (534 lines) — whose only job is to sit between the runner and the kernel for the instant in which a target process comes into existence: it reads a validated `LaunchSpec` from a private file, builds a Landlock ruleset, confirms the caller's cgroup resource limits, then starts the target so that the kernel `execve`s it already confined. It is a separate **process**, not a library, because the only irreversible, self-targeting primitive it needs is `Ruleset::restrict_self()`, which confines the calling process and every process it later creates: a library linked into `eggworkd` would either confine the long-lived daemon itself or have to confine the target after it is already running, leaving an unconfined window. It is therefore the single documented exception to process-ownership (see [Execution ownership](execution-ownership.md)): it may create the target process, and only for that reason, and only *after* the mechanics it applies were already decided elsewhere.

Crate facts: one `[[bin]]` (`path = "src/main.rs"`), `version.workspace = true`, no production dependency on any workspace crate, not linked into `eggworkd`. The whole implementation is one `#[cfg(target_os = "linux")] mod linux` — `entry`, `run`, `read_spec`, `restrict`, `exit_code`, `denied_environment_key`, `read_counter`, `verify_resource_limits`, plus the private `ResourceEvents`/`EventCounter` helpers — behind two `#[cfg]`-selected `main` functions. Five size constants bound everything it reads: `MAX_SPEC_BYTES` 1 MiB, `MAX_ARG_COUNT` 256, `MAX_ARG_BYTES` 32 KiB, `MAX_ENV_COUNT` 256, `MAX_ENV_NAME_BYTES` 256, `MAX_ENV_VALUE_BYTES` 16 KiB.

## Responsibility boundary

**Owns**

- Construction and self-application of a Landlock `workspace_rw` ruleset (ABI V4) over one workspace root plus a fixed runtime read allowlist.
- Pre-flight verification that the cgroup it is already inside actually enforces the `resource_*` values the spec declares (`memory.max`, `memory.swap.max`, `pids.max`, `cpu.max`).
- Observation, via pre-opened cgroup event files, of whether `oom_kill` or `pids.max` was hit while the target ran; reporting it to the runner as two status bits.
- Environment construction for the target: a cleared environment rebuilt from the spec, with `PATH` pinned.
- Reporting a structured result to the runner over a one-byte AF_UNIX status channel and translating the target's `ExitStatus` into an integer exit code.

**Does not own**

- **No execution admission.** It never decides whether a workload may run. The profile name, the resource values, the argv, the root, and the cwd were all chosen by `eggwork-runner` before the helper was spawned; `TrustedLandlockSetup::prepare` (`crates/eggwork-runner/src/lib.rs:425`) has already refused to hand over a `Required` request whose profile is not `workspace_rw` or whose helper fails `verify_trusted_helper`.
- **No lifecycle.** It does not time out, cancel, reap, or signal anything. Timeout/cancellation/termination of the *target* is the runner's, via the process group and `kill_on_drop` (`SandboxChannel::command`, `lib.rs:1779`). The helper only `wait()`s.
- **No policy decision about whether execution is allowed.** It applies mechanics that were already decided. Every branch that ends in `Err` is an abort, never a downgrade: nothing here can turn "required" into "best effort".
- **No trust self-assertion.** The helper never states that it is trusted; the runner verifies the file and the runner alone interprets the status bytes.

**Never does**

- Never falls back. There is no "run it anyway" branch, no `BestEffort` handling, and no "warn and continue" path anywhere in the crate.
- Never re-reads or re-derives the target's identity: `program`, `argv`, `cwd`, and `environment` are taken from the spec verbatim.
- Never touches anything outside its own cgroup: it reads `/proc/self/cgroup` and `/sys/fs/cgroup/...` read-only, and writes exactly one file — the unlink of the spec it already consumed.
- Never signals, kills, or times out anything. It has a `Child` and a `wait()`; it has no group, no alarm, and no escalation.
- Never accepts a different profile. `"workspace_rw"` is the only value `read_spec` will accept, and `None` is the only way to get no Landlock at all.

**Why apply-then-exec is the stronger shape.** Landlock's enforcement model is per-`task`, installed with `landlock_restrict_self` and inherited across `fork`/`clone` and preserved across `execve` (with `no_new_privs` required, which `restrict_self` sets). A library confined inside a daemon could not un-confine itself afterwards: `restrict_self` is one-way for that process, so the daemon would be permanently jailed. The alternative — a library that constrains an already-running target — is strictly weaker on two axes: the target executes attacker-controlled code during the pre-confinement window, and any privilege the target can acquire (a setuid child, a `ptrace`-capable child, a mount or user-namespace transition available in its own context) is available *before* the ruleset exists, whereas the ruleset inherited at `execve` time bounds what that privilege can reach. Confining the process that is about to `execve` means the kernel installs the confinement and the new program image in the same breath; there is no interval in which the target exists unconfined.

## Trust boundary and installation

The helper is treated as an installation-owned, trusted root (or effective-user) artifact. Trust is verified by the **runner**, in one function, `eggwork_runner::verify_trusted_helper` (`lib.rs:596`, Linux) — deliberately public so operator diagnostics report the same answer the enforcement path will give rather than a second, independently drifting copy of the policy (the doc comment says so explicitly). The Linux checks, in order:

| Check | Detail |
| --- | --- |
| Existence | `symlink_metadata` failure → `"sandbox helper is unavailable"` |
| File kind | must be a regular file and **not** a symlink (symlink itself → trust failure) |
| File owner | `uid ∈ {0, geteuid()}` |
| File mode | `mode & 0o022 == 0` (not group- or world-writable) and `mode & 0o111 != 0` (executable) |
| Parent dir | `path.parent().canonicalize()`; must be a directory, `uid ∈ {0, geteuid()}`, `mode & 0o022 == 0` |
| Every ancestor | must be a directory; `uid ∈ {0, geteuid()}` **or** a root-owned sticky directory (`uid == 0 && mode & 0o1000 != 0`); `mode & 0o022 == 0` unless it is that sticky root directory |

Any failure returns `Err(String)` and the caller reacts by request severity: `Required` → `RunnerError::Setup(reason)` (execution refused), `BestEffort` → `SandboxOutcome::NotApplied { reason }` (surfaced, never described as enforced). The non-Linux variant (`lib.rs:641`) is unconditional: `Err("trusted Landlock is unsupported on this platform")`. So required isolation fails closed rather than degrading.

`TrustedLandlockSetup::prepare` calls `verify_trusted_helper` **twice** in principle: once when resource setup was applied by `SystemdCgroupBackend` (a required-resource request refuses to continue with an untrusted helper; a best-effort one downgrades to `NotApplied`), and once for the sandbox profile. It never executes the helper during `prepare` — execution happens later, in `SandboxChannel::wait`.

The second, independent gate is the version-coherence probe, and it is not in the runner. `eggworkd deployment status` calls `check_helper_compatibility(helper_path, env!("CARGO_PKG_VERSION"))` (`crates/eggwork-server/src/deployment.rs:330` onward): a lighter duplicate of the ownership/mode/executable checks, then `eggup_core::CommandSpec::new(path).arg("--version").timeout(timeout)` run through `eggup_core::run_bounded` (cleared environment, null stdin, bounded output, timeout, kill/reap). The printed version must be non-empty, ≤ 64 bytes, free of control characters, and an **exact** match for the daemon's own crate version, otherwise `HelperCompatibility::VersionSkew { expected, found }`. `helper_satisfies_required_isolation(compatibility)` (`deployment.rs:300`) is what gates the advertised requirement. This probe is the documented ownership exception in [execution-ownership.md](execution-ownership.md): it "performs no execution admission, owns no lifecycle, and exists only so required-isolation admission can fail closed on version skew."

**Where the path comes from.** `TrustedLandlockSetup::discover_sibling()` (`lib.rs:412`) canonicalizes `std::env::current_exe()`, takes its parent, and appends `eggwork-sandbox-helper`, falling back to the bare relative name `eggwork-sandbox-helper`. The production daemon does not use that convenience: `start_server` (`operations.rs:1029`) constructs `TrustedLandlockSetup::new(helper)` from `config.sandbox_helper`, the operator-configured absolute path. Both funnel into the same `verify_trusted_helper`.

**Release-unit consequence.** `README.md`: "Linux releases ship the daemon and its Landlock sandbox helper as one bundle from one source revision, so the pair can never drift. macOS and Windows ship the daemon only." `install_unit_matrix(helper_required)` (`deployment.rs:119`) is the machine-readable form: `MEMBER_DAEMON` → `DEST_DAEMON` is always `required: true`; `MEMBER_HELPER` → `DEST_HELPER` (`bin/eggwork-sandbox-helper`) takes `required: helper_required`, and `eggworkd` sets that to `config.sandbox_helper.is_some()` (`src/bin/eggworkd.rs:193`) — i.e. a node configured with a helper path on Linux. `crates/eggwork-server/tests/release_contract.rs::the_helper_ships_exactly_on_linux_release_targets` pins the helper to the Linux triples only and asserts `install_unit_matrix(false)` marks it not required, which is the macOS/Windows shape. `operations.rs:449` (`doctor`) and `operations.rs:1029` (`start_server`) are the two operational consumers: `doctor` reports `trusted_helper(helper)` then, only if trusted, builds a `TrustedLandlockSetup` and asks for `execution_capabilities()`; `start_server` selects `TrustedLandlockSetup` when `config.sandbox_helper` is set and `NoExecutionSetup` otherwise.

## Command-line contract

Two accepted argv shapes, parsed strictly. Anything else is `Err(())` → exit **125**.

| Invocation | Behavior |
| --- | --- |
| `eggwork-sandbox-helper --version` (and *no* further argument) | prints `env!("CARGO_PKG_VERSION")` to stdout, one line, exit 0. `--version` with a trailing argument is an error, not a version print. |
| `eggwork-sandbox-helper --spec SPEC --status SOCK` (in that order, nothing else) | full launch path. Order is fixed: the second token must be `--status`, a fourth token must not exist. |

The runner therefore builds exactly (`SandboxChannel::command`, `lib.rs:1779`):

```
Command::new(<helper_path>)
    .arg("--spec").arg(<tempdir>/spec.json)
    .arg("--status").arg(<tempdir>/status.sock)
    .current_dir(<tempdir>)          // the spec's parent
    .stdin/stdout/stderr(Stdio::piped())
    .kill_on_drop(true)
    .env_clear()
    .env("PATH", "/usr/bin:/bin").env("LANG", "C.UTF-8").env("LC_ALL", "C.UTF-8")
```

`current_dir` is the `0700` temp directory, so the helper starts with the same working directory the spec file lives in and never depends on the daemon's cwd. `env_clear()` on the helper itself matters: the helper must not inherit the daemon's `LD_*`, `GIT_*`, or cloud variables while it is parsing a spec that a deny-list was supposed to keep clean.

The `--spec` path writes **nothing** to stdout or stderr. All structured output is one-byte codes on the connected `UnixStream`; the helper never logs. The runner gives the helper piped stdio (`SandboxChannel::command`, `lib.rs:1779`) and drains it with the ordinary capture machinery, so the only output an untrusted-looking helper could produce is bounded like any child's.

Order of operations inside `run()` (`main.rs:48`):

1. `UnixStream::connect(&status_path)` — **before** the spec is read. A missing or unconnectable socket aborts immediately, leaving the spec file untouched and no target started.
2. `read_spec(&spec_path)`; on failure write the numeric code and abort.
3. `fs::remove_file(&spec_path)` — the spec is consumed exactly once.
4. `verify_resource_limits(&spec)`; on failure write `2` and abort.
5. `ResourceEvents::new(...)` — opens `memory.events` / `pids.events` **before** restriction, recording baselines.
6. `restrict(&spec)`, only when `spec.profile.is_some()`; on failure write `2`/`30`/`31` and abort.
7. Write `1` (phase marker: mechanics verified), flush.
8. Build the `Command`, `spawn()`; on failure write `0` and abort.
9. Write `1` again (phase marker: target started), flush.
10. `child.wait()`, then write the resource bit-flags byte, then `exit_code(result)`.

**Bounded output and time.** The status protocol is a fixed one-byte-per-phase exchange; the runner wraps each phase read in `tokio::time::timeout(Duration::from_secs(5))` — `accept`, the first status byte, the second status byte — and the final resource byte in a 2 s timeout inside `SandboxSession::resource_limits_exceeded` (`lib.rs:1633`). `--version` is bounded by the deployment layer's `eggup_core::run_bounded` plus the ≤ 64-byte/no-control-character validation. Helper-side size bounds are the `MAX_*` constants in `read_spec`.

**Exit codes.** `entry()` is `std::process::exit(run().unwrap_or(125))`, so 125 means "the helper refused or could not proceed" for every cause, and is intentionally *not* a fine-grained channel: the reason is delivered on the status socket, and 125 is what remains when the socket could not even be connected. A successful run exits with the target's code: `exit_code(status) = status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))` — the conventional 128 + signal form for a signalled child.

**Non-Linux entry point** (`main.rs:393`): accepts `--version` alone (print version, exit 0) and otherwise exits 125. It does not link Landlock or nix and never starts anything.

## LaunchSpec

Read by `read_spec` (`main.rs:123`) from the path given by `--spec`. The runner writes it as `SandboxLaunchSpec` (`lib.rs:1605`) via `serde_json::to_vec` into `spec.json` inside a `0700` temp dir (`tempfile::Builder::new().prefix("eggwork-sandbox-")`), opened with `create_new(true)` and `mode(0o600)`; the encoded length is rejected above 1 MiB before the file is even created.

| Field | Type | Validation |
| --- | --- | --- |
| `schema_version` | `u16` | must be exactly `1` |
| `profile` | `Option<String>` | absent = no Landlock at all; present = must be `"workspace_rw"` |
| `root` | `PathBuf` | `canonicalize()` must succeed (code 13) and must equal the given path (code 15); must be a directory; then opened **once** with `O_PATH | O_CLOEXEC` and the descriptor is what `restrict` enforces (code 16 if that open fails) |
| `cwd` | `PathBuf` | `canonicalize()` must succeed (14); must start with `root` and be a directory (15) |
| `argv` | `Vec<String>` | non-empty, ≤ `MAX_ARG_COUNT` (256); every element non-empty, ≤ `MAX_ARG_BYTES` (32 KiB), and free of `\0` |
| `environment` | `Vec<(String, String)>` | ≤ `MAX_ENV_COUNT` (256); key non-empty, ≤ 256 bytes, free of `=` and `\0`; value ≤ 16 KiB and free of `\0`; `PATH` must be exactly `"/usr/bin:/bin"`; any other `denied_environment_key` match is refused; keys must be unique |
| `resource_memory_bytes` | `Option<u64>` (`#[serde(default)]`) | consumed by `verify_resource_limits` |
| `resource_cpu_millis` | `Option<u64>` (`#[serde(default)]`) | consumed by `verify_resource_limits` |
| `resource_pids` | `Option<u32>` (`#[serde(default)]`) | consumed by `verify_resource_limits` and `ResourceEvents` |

`#[serde(deny_unknown_fields)]` is set, so an unrecognized key is a parse error, not a silently ignored field.

As the runner writes it, a fully-populated spec is:

```json
{
  "schema_version": 1,
  "profile": "workspace_rw",
  "root": "/canonical/execution/root",
  "cwd": "/canonical/execution/root/subdir",
  "argv": ["/bin/sh", "-c", "printf ok > output"],
  "environment": [["PATH", "/usr/bin:/bin"], ["LANG", "C.UTF-8"], ["LC_ALL", "C.UTF-8"]],
  "resource_memory_bytes": 16777216,
  "resource_cpu_millis": 500,
  "resource_pids": 12
}
```

`root` is canonicalized by the runner before serialization (`lib.rs:1725`) and a failure there is `RunnerError::InvalidRoot`, so the helper's `root != spec.root` equality check (`code 15`) is a re-assertion, not the first line of defense. `resource_*` are populated only when the runner's cgroup backend actually applied them (`resources_applied.then_some(...)`, `lib.rs:1732-1749`): when the backend did not apply them, the fields serialize as `null` and the helper's verification short-circuits, so a `BestEffort` resource request that was not installed cannot make the helper refuse.

**The hand-off, end to end.**

1. Runner creates a `0700` temp dir, serializes the spec into `spec.json` with `create_new` + `mode(0o600)`, and binds `status.sock` with `UnixListener::bind` **before** spawning the helper — so the listening socket exists first and the helper's `connect` cannot lose a race.
2. Helper connects, stats and reads the spec, unlinks it.
3. Every remaining input the target needs travels in that one file, so the helper's own argv stays a fixed four tokens regardless of workload size or content.

**Status codes emitted by `read_spec`** (one byte, then abort with 125): `4` stat failed · `5` not a regular file / symlink / `uid != geteuid()` / `mode & 0o077 != 0` / `len > MAX_SPEC_BYTES` · `6` open failed · `7` fstat failed · `8` `dev`/`ino` mismatch between the pre-open stat and the opened descriptor (TOCTOU narrowing) · `9` read failed · `10` more than `MAX_SPEC_BYTES` read (the read is bounded by `take(MAX_SPEC_BYTES + 1)`) · `11` JSON parse failed · `12` semantic validation failed · `13` `root` canonicalize failed · `14` `cwd` canonicalize failed · `15` root/cwd containment failed · `16` the pinned root descriptor could not be opened · `2` resource verification failed · `30` ruleset not `FullyEnforced` · `31` `no_new_privs` not set · `0` target spawn failed.

**Why the spec is data, not argv.** The environment alone can be hundreds of KiB (`MAX_ENV_COUNT` 256 × `MAX_ENV_VALUE_BYTES` 16 KiB is a ~4 MiB envelope) and argv can be 256 × 32 KiB; the kernel's `MAX_ARG_STRLEN`/total argument limit is far below that, and `execve` of the helper itself would fail. More importantly, a file plus a schema removes an injection surface: the target's argv and environment are JSON string values, not text the helper has to re-parse or quote, and `read_spec` can reject NUL bytes and `=` in names that argv could only carry ambiguously. The one-shot `remove_file` plus the private `0700`/`0600` directory, the euid ownership requirement, and the dev/ino recheck after open make the hand-off a capability transfer rather than a shared file.

## Filesystem isolation (Landlock)

`restrict(&spec)` (`main.rs:186`), called only when `spec.profile.is_some()`:

```rust
let abi = ABI::V4;
let handled = AccessFs::from_all(abi);
let mut ruleset = Ruleset::default()
    .handle_access(handled)?
    .create()?
    .add_rule(PathBeneath::new(root_fd, handled))?;   // root_fd opened in read_spec
```

- **ABI**: hard-coded `ABI::V4`. There is no fallback and no downgrade: `handle_access`, `create`, every `add_rule`, every `PathFd::new`, and `restrict_self` all map to status code `2` on failure. On a kernel that does not implement V4 the helper refuses to start the target.
- **Handled rights**: `AccessFs::from_all(ABI::V4)` — the full handled set for that ABI, which is what makes the model "deny by default": anything not covered by a rule is denied.
- **Workspace rule is pinned to a descriptor, not a name.** `read_spec` opens `root` once, immediately after the containment checks, with `O_PATH | O_CLOEXEC` (mirroring what `PathFd::new` would have used) and hands the `File` to `restrict`, which passes it straight to `PathBeneath::new`. The object that was validated is therefore the object that is enforced. This used to re-resolve `spec.root` *by name* inside `restrict`, which left a window in which any process able to write the parent of the workspace root could replace it with a symlink; the ruleset would then grant all rights beneath the link target while still reporting `FullyEnforced`. Exploiting that needs a second process at the node's uid — the confined target cannot do it, because `workspace_rw` grants rights only *beneath* root — but it is exactly the adversary the spec file is already hardened against (reject symlink, then compare `dev`/`ino` on the descriptor actually read), and `root` was getting none of that discipline. `spec.cwd` is still resolved by name at spawn, but that is bounded: the pinned root rule denies everything outside the workspace regardless of where `chdir` lands.
- **Workspace rule**: the single `PathBeneath` rule on that pinned root descriptor with **all** handled rights, i.e. read *and* write *and* create *and* remove. This is the `workspace_rw` profile. The mapping from the public capability name to these concrete rights is: `LANDLOCK_WORKSPACE_RW_CAPABILITY` = `"isolation.landlock.workspace-rw.v1"` (`lib.rs:528`), advertised only when `verify_trusted_helper` succeeds **and** `probe_landlock_ruleset()` is true; the probe builds the identical `ABI::V4` ruleset with the identical four runtime paths and identical rights in-process but never calls `restrict_self` (`lib.rs:538` onward, including the comment that the devnull rule "must mirror the helper's null-device rule exactly").
- **Runtime read allowlist**: `for path in ["/usr", "/etc/ld.so.cache", "/etc/ssl/certs", "/dev/null"]`, each `canonicalize()`d, and a path that fails to canonicalize is **skipped silently** (no rule, so it stays denied):

  | Path | Rights granted |
  | --- | --- |
  | `/usr` (directory) | `AccessFs::from_read(abi) | AccessFs::Execute` — read plus execute so dynamically linked binaries run; no writes |
  | `/etc/ld.so.cache` (file) | `ReadFile` |
  | `/etc/ssl/certs` (directory) | `AccessFs::from_read(abi) | AccessFs::Execute` |
  | `/dev/null` | `ReadFile | WriteFile` |
  | anything else | denied |

  **`/dev/null` handling** (`main.rs:208-214`): granting write is deliberate and the in-source rationale is that the null device "discards all writes and yields EOF on reads: granting write is information-neutral, while denying it breaks ordinary `>/dev/null` redirection and makes escape-trap fixtures vacuous (redirection setup fails before the escape attempt runs). No other `/dev` path is allowed." That is exactly why the test fixture's `cat "$1" >/dev/null 2>&1` escape trap measures a real denial rather than a failed redirection.

- **Denied paths**: any path outside the ruleset — including the workspace root's parents, the target's own scratch space, `/tmp`, other `/dev` entries, `/proc` beyond the fds already open — is denied by the handled set, and `no_new_privs` blocks the target from acquiring privilege to escape.
- **`RestrictionStatus` outcomes**: `ruleset.restrict_self()` returns a `RestrictionStatus`; the helper requires `status.ruleset == RulesetStatus::FullyEnforced`, else status byte `30`, and requires `status.no_new_privs`, else status byte `31`. Both are aborts before the target exists — i.e. a partially enforced ruleset (a ruleset whose enforcement was degraded for a sub-layer) is treated as a refusal, not as best effort. The `RestrictionStatus` is otherwise discarded; it is not forwarded to the runner.
- **Ordering consequence**: because `restrict_self` is one-way for the calling process, the cgroup event files must be opened first — hence `ResourceEvents::new` precedes `restrict` in `run()`. After restriction, reading them still works because the descriptors were already resolved.
- **What the ruleset does not grant**: nothing outside the five `PathBeneath` rules, so the workspace's own parent, `/tmp`, `/var`, `/home`, `/run`, `/proc` (new opens), `/etc` beyond `ld.so.cache` and `ssl/certs`, and every other `/dev` node are denied. `AccessFs::from_read(abi)` is landlock's own aggregate; this crate composes it with `AccessFs::Execute` for directories and never enumerates individual bits, so the read-set membership is landlock's, not this crate's.
- **The `root` rule is the widest one.** Because the workspace root is granted `from_all(ABI::V4)`, a workspace containing a symlink or a hard link out of itself does not widen the ruleset — path resolution is what Landlock checks, which is exactly what test 3 of the suite demonstrates.

## Resource limits

**No rlimits are set anywhere in this crate.** There is no `setrlimit`/`rlimit` call. The helper's role is verification and observation; the limits themselves are applied by the runner's `SystemdCgroupBackend` (which wraps the helper command in a cgroup, and reports `ResourceSetupOutcome::Applied { backend: "systemd-cgroup-v2" }`), and this test asserts that backend string.

`verify_resource_limits(&spec)` (`main.rs:335`) is a pre-flight check that the cgroup the helper is *already inside* actually enforces the declared values, so the target is never started into a cgroup that silently does not bound what the request asked for. It returns `true` immediately when all three spec fields are `None`. Otherwise it reads `/proc/self/cgroup`, finds the `0::` unified-hierarchy line, and joins the remainder under `/sys/fs/cgroup` (an unreadable cgroup file, a missing `0::` line, or an unparseable value is a **failure**, via `is_none_or` / `return false` — fail closed):

| Declared | Files read | Acceptance rule |
| --- | --- | --- |
| `resource_memory_bytes` | `memory.max`, `memory.swap.max` | `memory.max` must parse as `u64` and be **≤** the declared value; `memory.swap.max` must be exactly `"0"` (swap must be off, or the memory bound is meaningless) |
| `resource_pids` | `pids.max` | must parse as `u32` and be **≤** the declared value |
| `resource_cpu_millis` | `cpu.max` | both whitespace-separated fields must parse as `u64`; `period != 0`; and `quota.saturating_mul(1000) / period <= cpu_millis` — i.e. the effective millicores-per-second must not exceed what was requested |

**The counter-file mechanism.** `ResourceEvents` (`main.rs:271`) holds up to two `EventCounter`s, one for memory and one for pids, each an open `fs::File` plus a key plus a baseline read at construction. `EventCounter::for_events` resolves `/proc/self/cgroup` → `<cgroup>/memory.events` (key `oom_kill`) or `<cgroup>/pids.events` (key `max`), and `self` is constructed with `track_memory.then(EventCounter::memory).flatten()` — so an unavailable cgroup file or an unparseable baseline simply means no tracking for that dimension, not a failure. `read_counter(&mut file, key)` (`main.rs:325`) seeks to 0, reads the whole file as a string, and returns the first line whose first space-separated token equals `key`, parsed as `u64`. `exceeded()` is `read_counter(...) > baseline` — a strictly-greater comparison, so a target that merely ran near the limit is not reported.

After the target exits, `ResourceEvents::status_code()` packs the result into one byte: bit 0 = `oom_kill` exceeded, bit 1 = `pids.events: max` exceeded. The runner decodes exactly these two bits in `SandboxSession::resource_limits_exceeded` into `eggwork_core::ResourceDimension::{Memory, Pids}` under a 2 s timeout, and a timeout or short read yields no exceeded dimensions rather than an error.

| Tracked when | File | Key | Reported as |
| --- | --- | --- | --- |
| `resource_memory_bytes.is_some()` | `<cgroup>/memory.events` | `oom_kill` | bit 0 → `ResourceDimension::Memory` |
| `resource_pids.is_some()` | `<cgroup>/pids.events` | `max` | bit 1 → `ResourceDimension::Pids` |
| `resource_cpu_millis` only | — | — | nothing; CPU exhaustion is observed by the runner's timeout, not by a counter |

`cpu_millis` is the one declared dimension with no counter: exceeding `cpu.max` shows up as a throttled process that the runner's timeout eventually terminates, and the test suite reflects that by asserting a CPU request runs to completion with `Applied` rather than by asserting a `LimitExceeded` classification.

Note the different units on the two mechanisms: the pre-flight check compares *cgroup file values* (what is enforced) against the *spec declaration* (what was requested), while the counters compare *event deltas over the target's lifetime* (what actually happened). Nothing here bounds the target by itself.

## Environment handling

`denied_environment_key` (`main.rs:242`) uppercases the key and matches it against a fixed list by exact equality **or** prefix: `LD_`, `DYLD_`, `PATH`, `IFS`, `SHELLOPTS`, `CDPATH`, `BASH_ENV`, `ENV`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS`, `RUBYOPT`, `PERL5OPT`, `GIT_ASKPASS`, `SSH_ASKPASS`, `GIT_CONFIG`, `GIT_CONFIG_`, `AWS_`, `AZURE_`, `GOOGLE_`, `SSH_AUTH_SOCK`. The intent is to strip every knob that would re-inject code or redirect the loader/interpreter inside a confined target: dynamic-linker search paths, shell startup files, interpreter startup options, and credential/config discovery for the major cloud and Git/SSH clients.

It is enforced twice, as two independent layers. The runner filters with its own copy of the same predicate before building the spec (`lib.rs:1465`, used at `lib.rs:1709`) so denied keys are not even written. The helper *validates* the spec: in `read_spec`, `denied_environment_key(key) && key != "PATH"` is a rejection (code `12`), and `PATH` is the single exception, permitted only with the exact value `"/usr/bin:/bin"`. Duplicate keys are also rejected.

| Group | Denied keys | Why |
| --- | --- | --- |
| Dynamic loader | `LD_`, `DYLD_` | a `LD_PRELOAD`/`LD_LIBRARY_PATH` in a confined process re-introduces attacker-chosen code and a path channel out of the ruleset |
| Shell | `IFS`, `SHELLOPTS`, `CDPATH` | word splitting, shell option state, and `cd` search paths are set from the environment *before* a script runs, so a confined process inherits the daemon's shell behavior unless these are stripped |
| Shell startup files | `BASH_ENV`, `ENV` | a login/non-interactive shell sources these automatically; they are a file-execution primitive pointed outside the workspace |
| Interpreter options | `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS`, `RUBYOPT`, `PERL5OPT` | the same class of injection, per runtime; `NODE_OPTIONS` alone is a code-execution vector |
| VCS / credential discovery | `GIT_ASKPASS`, `SSH_ASKPASS`, `GIT_CONFIG`, `GIT_CONFIG_`, `SSH_AUTH_SOCK` | these point an otherwise-sandboxed process at an agent or a config file outside the workspace |
| Cloud credentials | `AWS_`, `AZURE_`, `GOOGLE_` (prefixes) | a confined process must not inherit cloud credential endpoints or tokens from the daemon's environment |
| Path | `PATH` | permitted only as the single hard-coded value `"/usr/bin:/bin"`; see below |

**The surviving environment** is therefore a closed set that the helper reconstructs from scratch: `Command::env_clear()`, then `.env("PATH", "/usr/bin:/bin")`, then `.envs(...)` over the spec's environment with `key != "PATH"` filtered out — the `PATH` filter is redundant with the spec validation and exists so a pinned `PATH` cannot be overridden by a later entry. Nothing is inherited: the runner also launches the helper itself with `env_clear()` plus `PATH`/`LANG`/`LC_ALL` (`lib.rs:1791`). The spec the runner builds starts from the baseline `PATH`, `LANG=C.UTF-8`, `LC_ALL=C.UTF-8`, `CI=1`, `NO_COLOR=1`, `TERM=dumb`, `GIT_TERMINAL_PROMPT=0`, `PAGER=cat`, then appends non-denied request environment, then `EGGWORK_EXECUTION_ID` and `EGGWORK_EXECUTION_GENERATION` from `ExecutionProvenance` when present (`lib.rs:1698-1721`).

## Hand-off to the target

```rust
let (program, argv) = spec.argv.split_first().ok_or(())?;
let mut command = Command::new(program);
command.args(argv).current_dir(&spec.cwd).env_clear()
    .env("PATH", "/usr/bin:/bin")
    .envs(spec.environment.iter().filter_map(|(k, v)| (k != "PATH").then_some((k, v))));
let mut child = match command.spawn() { Ok(child) => child, Err(_) => { let _ = status.write_all(&[0]); return Err(()); } };
```

The step is `std::process::Command::spawn`, not a self-`execve`. That is a deliberate, observable difference from a "replace myself" helper: the helper must stay alive to `wait()` on the target, read the post-mortem cgroup counters, and return the target's status, so it cannot hand its own PID to the target. The security property still holds and is the same one, because the kernel's `execve` of the target happens in a child that already has the ruleset and `no_new_privs` installed — Landlock is inherited across fork and preserved across `execve`, so the new program image is confined from its first instruction. The helper's own continued existence is not a hole: it is the *more* restricted process, and its only remaining action is to wait and report.

**Order of operations: verify resources → open counters → restrict → spawn.** The spec's argv is reconstructed by `split_first` (program) plus `.args(rest)`; an empty `argv` is already rejected in `read_spec`, and the `ok_or(())` is belt-and-braces. `current_dir(&spec.cwd)` uses the canonicalized, containment-checked path.

**Failure handling is abort-only, always before the target exists.** Every failing step writes its code and returns `Err(())` → exit 125, before any `Command::spawn`:

| Failure | Status byte | Target started? |
| --- | --- | --- |
| status socket unconnectable | none written | no (spec not even read) |
| `read_spec` rejection | 4–15 | no |
| `verify_resource_limits` false | 2 | no |
| `restrict` failure | 2 | no |
| ruleset not `FullyEnforced` | 30 | no |
| `no_new_privs` unset | 31 | no |
| `argv` empty after validation | none | no |
| `spawn` itself fails (e.g. ENOENT on a denied or missing program) | 0 | no — this is the one code the runner treats as `AfterTarget(Spawn)` |

The runner's reading of those bytes (`SandboxChannel::wait`, `lib.rs:1798`): the first byte `0 | 2 | 3 | 30 | 31` → `Ok(None)` (no resource-reporting session; the run continues), `>= 4` → `Err(SandboxWaitError::BeforeTarget(RunnerError::Setup("sandbox helper rejected its launch specification (status {code})")))`, `1` → read the second byte: `1` → `Ok(Some(SandboxSession))`, `0` → `AfterTarget(Spawn("sandbox target could not start"))`, anything else → `"sandbox status was invalid"`. Every arm is a single byte read, never a length-prefixed or textual protocol.

The wire sequence, as a fixed four-message exchange with no framing beyond "one byte":

| # | Direction | Byte | Meaning |
| --- | --- | --- | --- |
| 1 | helper → runner | accept | the helper reached `connect` and passed argv parsing |
| 2 | helper → runner | `4`–`15` | spec rejected; runner errors before spawn |
| 3 | helper → runner | `2` / `30` / `31` | mechanics failed; run continues without a resource session |
| 4 | helper → runner | `1` | resources verified and Landlock applied (or not requested) |
| 5 | helper → runner | `1` | the target `execve` succeeded |
| 6 | runner → helper | — | runner now owns lifecycle; it waits for the helper's own exit |
| 7 | helper → runner | bit-flags byte | limit breaches observed (read under a 2 s timeout) |

Steps 4 and 5 deliberately reuse the value `1`: the runner distinguishes them by *position* in the sequence, not by value. The final flags byte is only read if step 5 succeeded, which is why a `SandboxSession` exists at all.

## Platform support

Linux is the only functional path, and the gate is on the module, not the dependency list: `#[cfg(target_os = "linux")] mod linux` holds `run`, `read_spec`, `restrict`, `exit_code`, `denied_environment_key`, `read_counter`, `verify_resource_limits`, plus `ResourceEvents`/`EventCounter`. Two `main` functions are `#[cfg]`-selected: the Linux one calls `linux::entry()`; the `#[cfg(not(target_os = "linux"))]` one (a) prints the version and exits 0 for a bare `--version`, and (b) exits 125 for everything else.

The crate still compiles on every other host because the machinery's dependencies are target-scoped (`landlock` under `cfg(target_os = "linux")`, `nix` under `cfg(unix)`), so a macOS or Windows build of the workspace pulls neither into the graph and the non-Linux entry point needs neither. What a non-Linux host actually gets is a binary that answers `--version` and refuses every launch with 125; the `[[bin]]` is unconditional in the manifest, so a host build of the workspace does produce that binary — it simply never becomes part of a release (the release file set for macOS and Windows is the daemon alone; `README.md` "Published targets" and `release_contract.rs` agree on the same split).

**Which layer refuses what — be precise.** Two different layers refuse, for two different reasons, and they are not interchangeable:

- The *runner* refuses at the API layer. `verify_trusted_helper`'s non-Linux variant returns `Err("trusted Landlock is unsupported on this platform")` unconditionally, so `TrustedLandlockSetup::prepare` turns a `Required` request into `RunnerError::Setup` and a `BestEffort` request into `NotApplied`; `probe_landlock_ruleset`'s non-Linux variant is `false`, so the capability is never advertised; and the `SandboxChannel`/`SandboxSession`/spec-serialization code in the runner is itself `#[cfg(target_os = "linux")]`. On non-Unix hosts, [execution-ownership.md](execution-ownership.md) records the documented behavior: the runner returns an explicit unsupported-platform error until a process-tree backend is implemented and host-qualified.
- The *helper* refuses at the process layer, and only as a backstop: 125 with no status channel. The runner is expected to have refused first, so in a correct deployment the helper's non-functional path is unreachable.

`nix` is used in exactly one place in the helper — `nix::unistd::geteuid()` in `read_spec`'s spec-ownership check (the `user` feature) — under `cfg(unix)`, so macOS compiles the check but never reaches the sandbox path.

## Dependencies

| Dependency | Scope | Why |
| --- | --- | --- |
| `serde` (derive `Deserialize`) | always | `LaunchSpec` |
| `serde_json` | always | spec parsing in `read_spec` |
| `landlock` | `cfg(target_os = "linux")` | the only user is `mod linux`; target-scoped so it never enters the build graph on other hosts |
| `nix` (`features = ["user"]`) | `cfg(unix)` | `geteuid()` for the spec ownership check |

The `Cargo.toml` states the intent in a comment: "The helper's Linux-only sandbox machinery (`mod linux`) is the sole user of these crates. Both are Unix/Linux-only and must not enter the build graph on other hosts; the non-Linux entry point uses neither."

Dev-dependencies are `eggwork-runner`, `eggwork-core`, `tempfile`, `tokio` (`features = ["net"]`), and `tokio-util` — used only by `tests/landlock_runner.rs`, which drives the *real* binary (`env!("CARGO_BIN_EXE_eggwork-sandbox-helper")`) through the real `LocalProcessRunner`. None of them enter the shipped binary's graph, which is what keeps the trusted artifact's dependency surface to serde/serde_json plus one sandbox crate. Combined with `#![forbid(unsafe_code)]`, the shipped helper contains no `unsafe` of its own.

## Test coverage

`tests/landlock_runner.rs` is 534 lines, 10 tests, **all `#[cfg(target_os = "linux")]`** (9 `#[tokio::test]`, 1 `#[test]`); on any other host the file compiles and runs zero tests. Every test copies `CARGO_BIN_EXE_eggwork-sandbox-helper` into a `0700` temp dir as `0755` so `verify_trusted_helper` accepts it — the trust rules are satisfied by construction, which is itself part of the contract being tested. The suite is an integration suite: it never imports the helper, it drives the real binary either through the real `LocalProcessRunner` or, once, through a raw `Command`.

| Test | What it pins down |
| --- | --- |
| `required_landlock_allows_workspace_and_denies_outside_reads_and_writes` | rw workspace, denied outside read (`exit 77`), denied outside write (`exit 78`), `Applied` outcomes, request `PATH` neutralized |
| `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers` | trust-check failure modes, `BestEffort` → `NotApplied` + exit 0, `Required` → error + marker never created |
| `workspace_symlink_cannot_read_outside_workspace` | a symlink out of the root does not become a read channel |
| `cwd_symlink_outside_workspace_is_rejected_before_launch` | `RunnerError::InvalidWorkingDirectory` from the runner, before the helper exists |
| `sandbox_timeout_and_cancellation_reap_the_helper_process_group` | advertised capabilities, `TimedOut`, `Cancelled`, grandchild reaped or zombie |
| `required_memory_limit_is_enforced_and_classified` | `ResourceLimit` + `LimitExceeded` for memory, via the `oom_kill` counter |
| `required_pid_limit_is_enforced_and_classified` | `ResourceLimit` + `LimitExceeded` for pids, via the `pids.events` counter |
| `required_cpu_quota_is_verified_before_target_start` | `cpu.max` accepted by the pre-flight check, backend `systemd-cgroup-v2` |
| `concurrent_resource_scopes_keep_pid_limits_isolated` | two concurrent runs get independent cgroups, so one run's breach is not attributed to the other |
| `malformed_private_spec_and_unavailable_status_channel_do_not_launch_target` | direct binary invocation: status `12` for a bad profile, and abort-before-connect; the target marker is never created |

In detail:

1. `required_landlock_allows_workspace_and_denies_outside_reads_and_writes` — `/bin/sh` reads `input`, writes `output` (both inside the root), and runs two escape traps: `exit 77` if a file outside the root can be read, `exit 78` if a path outside can be created. Asserts exit 0, stdout `inside\n`, `output` == `ok`, `outside-write` absent, and `SandboxOutcome::Applied` / `eggwork_core::SandboxResult::Applied`. Also sets `PATH=/malicious/path` and gets exit 0 — the request's `PATH` is filtered by the runner, so the helper sees only the pinned baseline. This is also the only exercise of the `/dev/null` rule, via the trap's `>/dev/null 2>&1`.
2. `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers` — `prepare` with `Required` returns `Err` for a missing path, a non-executable `0644` file, and a symlink to the good helper; `BestEffort` with a missing helper returns `Ok(NotApplied)`; a `BestEffort` run of `/bin/true` exits 0 with `NotApplied`; and a `Required` run with a missing helper errors with the marker file `required-must-not-run` **not** created.
3. `workspace_symlink_cannot_read_outside_workspace` — a symlink inside the root pointing at an outside secret, `/bin/cat escape`: non-zero exit, empty stdout, `SandboxOutcome::Applied`.
4. `cwd_symlink_outside_workspace_is_rejected_before_launch` — a working directory that is a symlink out of the root yields `RunnerError::InvalidWorkingDirectory` from the runner, before the helper is involved.
5. `sandbox_timeout_and_cancellation_reap_the_helper_process_group` — asserts `execution_capabilities()` contains `isolation.landlock.workspace-rw.v1`, `resources.cgroups-v2.memory`, `.cpu`, `.pids`; a 100 ms timeout on `sleep 30` yields `TerminationReason::TimedOut`; cancelling a `sleep 30 & wait` run yields `Cancelled` and the grandchild is gone or a zombie.
6. `required_memory_limit_is_enforced_and_classified` — 16 MiB requested, python3 touches 64 MiB: `ExecutionFailure::ResourceLimit` and `ResourceDimensionResult::LimitExceeded` for memory. Uses `SandboxRequest::None`, so the helper runs with `profile: None` (no Landlock) purely as the resource-verification/reporting path.
7. `required_pid_limit_is_enforced_and_classified` — `pids: Required(12)` against 64 forks: `ResourceLimit` + `LimitExceeded` for pids.
8. `required_cpu_quota_is_verified_before_target_start` — `cpu_millis: Required(500)` and a trivial command: stdout `quota-ok` and cpu `Applied { backend: "systemd-cgroup-v2" }`, i.e. the pre-flight `cpu.max` check accepted the installed quota.
9. `concurrent_resource_scopes_keep_pid_limits_isolated` — two overlapping runs on the same `LocalProcessRunner`, one with `pids: Required(6)` and one with `Required(32)`, each forking 8 children: the low scope reports `LimitExceeded` while the high scope reports `Applied`. This is the test that keeps the shared-cgroup hazard in `read_counter` honest.
10. `malformed_private_spec_and_unavailable_status_channel_do_not_launch_target` — the only **direct** invocation of the binary. A `0700` dir, a `0600` `spec.json` with `profile: "unsupported-profile"` and `argv: ["/usr/bin/touch", marker]`, a bound `status.sock`: the first status byte is exactly `12`, the child exits non-zero, and the marker is absent. Second case: a valid `workspace_rw` spec but a missing status socket — non-zero exit, marker absent, proving the connect happens before the spec is read.

**Thin areas, honestly.** The `restrict` failure codes `30` and `31` have no test, and neither does `2` (resource verification refused) — so the "Landlock not fully enforced" and "`no_new_privs` unset" branches are unexercised. `read_spec` codes `4`, `5` (e.g. a `0644` or euid-mismatched spec), `6`–`11`, `13`–`15` are untested; only `12` is. There is no test for oversized argv/environment or a > 1 MiB spec, none for `denied_environment_key` at the helper level (test 1's `PATH` is neutralized by the runner, not refused by the helper), none for `exit_code`'s 128 + signal mapping, and none that invokes the helper's `--version` path directly. All 9 tests are Linux-gated, so the non-Linux 125 path has no coverage. The memory/PID tests assume `python3` at `/usr/bin/python3` and a cgroup v2 hierarchy the test host can write.

## Invariants and enforcement

| Invariant | Enforcement site | Test |
| --- | --- | --- |
| A required sandbox profile is never served by an untrusted helper | `verify_trusted_helper` → `RunnerError::Setup` (`lib.rs:465-473`) | `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers` |
| Best-effort sandbox is reported as `NotApplied`, never as enforced | `SandboxOutcome::NotApplied { reason }` (`lib.rs:469`) | same test |
| Workspace is read-write, everything else outside the runtime allowlist is denied | `restrict`, `ABI::V4`, `from_all` handled set, root rule at full rights (`main.rs:186`) | `required_landlock_allows_workspace_and_denies_outside_reads_and_writes`, `workspace_symlink_cannot_read_outside_workspace` |
| `/dev/null` stays writable; no other `/dev` path is allowed | explicit `/dev/null` rule + comment (`main.rs:208`) | exercised implicitly by the escape trap's `>/dev/null 2>&1` |
| A partially enforced ruleset is a refusal, not a downgrade | `status.ruleset != FullyEnforced → 30` (`main.rs:227`) | none (uncovered) |
| `no_new_privs` is set | `!status.no_new_privs → 31` (`main.rs:230`) | none (uncovered) |
| No ABI downgrade to an older Landlock version | hard-coded `ABI::V4`, every failure → 2 | indirect: runner's `probe_landlock_ruleset` gates the capability |
| The target never starts with the declared limits unenforced | `verify_resource_limits` (`main.rs:335`), fail-closed on unreadable/unparseable | `required_cpu_quota_is_verified_before_target_start`; memory/pids classification tests |
| Limit breaches are attributed per dimension, not guessed | `ResourceEvents` bit flags → `ResourceDimension::{Memory, Pids}` | `required_memory_limit_is_enforced_and_classified`, `required_pid_limit_is_enforced_and_classified` |
| Per-run cgroup isolation is real, not shared state | runner-side scope creation; helper only reads its own `/proc/self/cgroup` | `concurrent_resource_scopes_keep_pid_limits_isolated` |
| A malformed spec aborts before the target runs | `read_spec` → code 12 + `Err` | `malformed_private_spec_and_unavailable_status_channel_do_not_launch_target` |
| An unavailable status channel aborts before the spec is read | `UnixStream::connect` first (`main.rs:69`) | same test |
| The environment is rebuilt, never inherited | `env_clear()` + pinned `PATH` + validated spec env (`main.rs:100`) | `required_landlock_allows_workspace_and_denies_outside_reads_and_writes` (`PATH=/malicious/path`) |
| Loader/interpreter/credential knobs never reach the target | `denied_environment_key` in both crates | none directly (runner-side filter is what test 1 observes) |
| Timeout and cancellation reap the whole tree | runner process-group signaling; helper only `wait()`s | `sandbox_timeout_and_cancellation_reap_the_helper_process_group` |
| Helper and daemon cannot be version-skewed | exact `--version` match in `check_helper_compatibility` | `crates/eggwork-server`: `helper_version_skew_is_detected`; `release_contract::the_helper_ships_exactly_on_linux_release_targets` |

## Review focus

- **Every failure path is before the target, not after.** Re-derive the 8 abort points in `run()` and confirm none of them can fall through to `Command::spawn`. In particular the ordering `verify_resource_limits` → `ResourceEvents::new` → `restrict` → `spawn` is load-bearing: moving `ResourceEvents::new` after `restrict` would break counter reads, and moving `restrict` after the `Command` is built but before `spawn` is safe only because the build itself opens nothing.
- **The status-byte contract has a gap worth confirming.** The runner treats first byte `0 | 2 | 3 | 30 | 31` as `Ok(None)` — run continues, no resource session — and the helper only ever writes `1` at that position. So a helper that cannot fully enforce Landlock (`30`), cannot set `no_new_privs` (`31`), or fails resource verification (`2`) lets the target start anyway with only resource reporting disabled; the runner has already decided severity in `prepare`, so this may be intended, but it is the single place where a *runtime* enforcement failure is not turned into a run failure. Decide whether that is the intended policy.
- **`ABI::V4` with no fallback.** On a kernel older than Landlock V4 the helper refuses (code 2) and the target does not start, while `probe_landlock_ruleset` would already have withheld the capability. Confirm the two paths agree on every kernel the release claims to support, and that "refuse" rather than "degrade to V1/V2" is the accepted behavior.
- **Implicit deny is doing the work.** `AccessFs::from_all(ABI::V4)` with only five rules means any *new* runtime path the target needs (a different loader cache, `/lib`, `/proc/self/maps`, `/etc/resolv.conf`, `$HOME`) is denied by default. That is the safe direction, but it makes the runtime allowlist a compatibility surface that must be reviewed whenever the runner is given a new argv shape. Also note `canonicalize()` failures silently skip a path — a permissions change on `/usr` would silently drop the rule rather than fail.
- **"Resource limits" are cgroup checks, not rlimits.** The crate sets none. The helper trusts that the runner put it in the right cgroup and *verifies* the files. A cgroup that is moved between `verify_resource_limits` and the counters' reads, or a `memory.max` that is tightened below the declared value afterwards, is not detected; conversely the check will refuse to start whenever `memory.swap.max != "0"`, which is a strict requirement on the runner's cgroup setup.
- **Counter baselines and delta semantics.** `exceeded()` is strictly-greater against a baseline captured *before* the target starts, from a shared cgroup. If the runner ever places two targets in one cgroup, one target's OOM kill would be attributed to the other. The `concurrent_resource_scopes_keep_pid_limits_isolated` test asserts the current per-run scoping; keep it that way.
- **Spec TOCTOU surface.** `read_spec` uses `symlink_metadata`, then `open`, then compares `dev`/`ino` — but it does not use `O_NOFOLLOW`, so the *opened* file is not proven to be the stat'ed non-symlink. The euid + `mode & 0o077 == 0` + `0700` parent requirements are what actually make the substitution uninteresting; confirm that is the intended layering rather than an oversight.
- **Could an untrusted caller invoke the binary directly?** Yes — argv is unvalidated input to any process, so `verify_trusted_helper` protects the *runner*, not the binary. What stops a malicious direct caller is narrow: the spec must be owned by the caller's euid and `0600`, the caller must be able to bind the status socket (self-reporting only, so it can fabricate any code), and the target will be confined to the caller's own `root` with the caller's own environment. A direct caller therefore gets a *correctly confined process with attacker-chosen rules*, i.e. no privilege gain — but it is worth confirming there is no path where a caller-supplied `root` escapes containment, since `read_spec`'s `root != spec.root` equality after canonicalization plus `cwd.starts_with(root)` is the only thing standing between the two.
- **Uncleared fd surface.** The helper holds the spec `File`, the `status` `UnixStream`, and up to two cgroup event `File`s across the target's lifetime, plus the `Child`'s inherited pipes. None are `CLOEXEC`-hardened in this crate, and the target inherits whatever the runner's `Command` set. Confirm the runner's stdio wiring is the intended boundary and that the cgroup event fds are harmless to leak (they are pre-restriction opens, which is also exactly why they keep working).
- **The environment denylist is prefix-matched and duplicated.** `denied_environment_key` lives in both crates and is a closed list; a new interpreter or loader knob (e.g. `PERL5LIB`, `RUBYLIB`, `NODE_PATH`, `GLIBC_TUNABLES`, `PROMPT_COMMAND`) is not covered. The duplication is deliberate (runner filters, helper validates) but is two copies to keep in sync, and only the runner's copy has any test coverage.
- **Exit code fidelity.** `exit_code` maps a signalled target to `128 + signal`, which is conventional but indistinguishable from a target that legitimately exited with that number. Worth confirming the runner's classification does not treat large exit codes as failures by accident.

## Related

- [Architecture overview](overview.md)
- [Runner execution](runner-execution.md)
- [Execution ownership](execution-ownership.md)
- [Deployment lifecycle](deployment-lifecycle.md)
