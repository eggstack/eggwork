# eggwork-runner

`eggwork-runner` is the sole owner of finite process lifecycle in the repository: the only component permitted to create OS child processes, terminate them, and reap their process groups. Everything about an execution's *runtime* — argv, cwd confinement, environment sanitization, stdin, bounded capture and streaming, timeout, cancellation, descendant cleanup, termination classification, cleanup diagnostics — happens in `LocalProcessRunner::run`. The embedding contract is two calls wide: build a validated `RunnerRequest` from an `eggwork_core::ExecutionSpec` with `RunnerRequest::from_spec`, then hand it to `LocalProcessRunner::run` together with a `CancellationToken` and an output `mpsc::Sender<OutputChunk>`. Platform isolation is injected, not hardcoded, through the `ExecutionSetup` trait; the runner carries no scheduler placement, priority, queue, or fairness policy.

Source of truth: [`crates/eggwork-runner/src/lib.rs`](../crates/eggwork-runner/src/lib.rs) (2,336 lines), [`crates/eggwork-runner/tests/capability_probe.rs`](../crates/eggwork-runner/tests/capability_probe.rs), [`crates/eggwork-runner/Cargo.toml`](../crates/eggwork-runner/Cargo.toml).

## Responsibility boundary

**Owns**

- OS child creation, process-group isolation, and process-tree termination/reaping.
- The full environment contract: clearing the inherited environment and rebuilding it from the node's own baseline under the shared environment policy — a deny-list filter, a node-owned-key rule, and one builder shared by both launch paths.
- Bounded stdin writes and concurrent stdout/stderr drain with head/tail retention and total/omitted byte accounting.
- Timeout, cancellation, and terminate-on-overflow races, and the resulting `TerminationReason`.
- Sandbox and OS resource-control *request/outcome* plumbing, plus the runtime capability probe that backs node feature advertisement.
- Translation of runner facts into the protocol-neutral `ExecutionResult` (`RunnerResult::execution_result`, `impl From<RunnerResult> for ExecutionResult`).

**Deliberately does not own**

- Global scheduling, placement, node selection, retry, or queueing. The crate doc states this explicitly: "neither this API nor its result types carry scheduler placement, priority, or project fairness policy" (the crate-level `//!` block). ADR-0005 assigns those to the server.
- Local admission / concurrency permits. `run` has no notion of a slot, drain state, or storage headroom.
- Artifact capture, artifact retention, and finalization. `RunnerResult::execution_result` hardcodes `artifact_count: 0` and `finalization_failure: None`.
- Network policy. `eggwork_core::CommandSpec::network` is destructured away by `from_spec` with `..` and never consulted.
- Declared outputs. `declared_outputs` is likewise discarded by `from_spec`; the server owns output capture.
- Transport, auth, and leases. `CancellationToken` is the *only* lifetime signal the runner accepts; lease policy is the caller's.
- The execution identity/digest/fencing rules of ADR-0003. It carries `ExecutionProvenance` opaquely into the child environment; it does not enforce generations.

The normative statement lives in [execution-ownership.md](execution-ownership.md): `eggwork-runner` is the canonical owner of finite-process lifecycle; `eggwork-sandbox-helper` may create the target process only to apply installation-owned sandbox/resource mechanics before launch. `scripts/check_execution_ownership.py` encodes the allowlist as exactly `{"eggwork-runner", "eggwork-sandbox-helper"}` and fails any other crate that constructs a process.

**What a caller cannot do**

Every validated field of `RunnerRequest` is private (`struct RunnerRequest`). There is no public field and no way to obtain a `&mut` to any of them. The public surface is a builder (`RunnerRequest::new` plus `set_*`) and `RunnerRequest::from_spec`; accessors exist for `sandbox_request()` and `resource_setup_request()`, and `resource_setup_request_mut()` is the single mutable door, scoped to the `Requirement` triple so it cannot weaken an unrelated field.

Private fields are necessary but not sufficient: a public constructor is a construction path too, and a request that never passes through the domain contract can be built through one. That is why the runner does not restate the contract at all — it *consumes* it:

1. **The rules have one owner, in `eggwork-core`.** The argv bounds (`eggwork_core::validate_argv`), the environment-entry rules including the duplicate-name rule (`eggwork_core::validate_environment_entries`), the timeout (`eggwork_core::validate_timeout`), the stdin bound (`eggwork_core::StdinPolicy::validate`), the output bounds (`eggwork_core::OutputPolicy::validate`) and the resource bounds (`eggwork_core::ResourceRequirements::validate`) are all core functions. The runner calls them; it does not re-derive them. `eggwork_core::CommandSpec::validate` calls the same functions, so the protocol path and the runner path cannot disagree.
2. **Every public construction path is covered.** `RunnerRequest::contract_error()` reports the first violation in a request without running anything, and `RunnerRequest::validate` — the private enforcement point, called at the top of `run`, before setup, before any helper, before `spawn` — refuses with the same typed error. There is no "already validated" flag to trust and no door into a request that the contract does not cover.
3. **The filesystem boundary is enforced by canonicalization, not by string prefix tests.** `validate` canonicalizes `root` and then canonicalizes `root.join(cwd)` and requires `canonical.starts_with(&root)`. A `..` traversal, a symlink, or a bind-like path is caught after resolution, not before. These two rules — the root-directory and working-directory resolutions — are the runner's own; `eggwork-core` does not own them, because it never sees a filesystem.
4. **Making fields public would make the validated state a convention.** The crate-level doc's claim "Request fields remain private so callers cannot bypass protocol bounds" is only true while the fields are private; `#![forbid(unsafe_code)]` plus workspace `unsafe_code = "deny"` means the same guarantee cannot be re-opened from inside either.

The runner is allowed to be **stricter** than its owner, and it is exactly once stricter: it refuses an empty argv entry, which core permits. That rule is the runner's own — an empty argument is a child-process construction hazard rather than a protocol concern — it is written out as `RunnerRequest::argv_entry_violation` with that reasoning in the source, and it is pinned by `the_runner_refuses_an_empty_argv_entry_which_core_permits`.

The same discipline appears in the `Debug` implementations. `RunnerRequest`, `OutputChunk`, and `BoundedCapture` have hand-written `Debug` impls that print `argv` as `[REDACTED]`, output bytes as `[REDACTED]`, and only `environment_count` for the environment; the stdin policy's `Debug` is core's, which redacts the bytes and prints only their length. A default derived `Debug` would have leaked argv, tokens, and child output into server logs.

## The embedding API

The crate doc (`//!` block) is the four-step contract:

1. Construct a validated `RunnerRequest` from an `eggwork_core::ExecutionSpec` with `RunnerRequest::from_spec`.
2. Run it through `LocalProcessRunner`.
3. Request fields remain private, and every public construction path is covered by the `eggwork_core` domain contract.
4. Platform setup is supplied through `ExecutionSetup`; neither this API nor its result types carry scheduler placement, priority, or project fairness policy.

### `RunnerRequest`

| Private field | Type | Source in `from_spec` | Rule owner | Checked by |
| --- | --- | --- | --- | --- |
| `argv` | `Vec<String>` | `CommandSpec::argv` | `eggwork-core` — `validate_argv`: non-empty, `len <= MAX_ARG_COUNT` (256), each `<= MAX_ARG_BYTES` (32 KiB), no `\0` | `RunnerRequest::validate` |
| `argv` (extra) | `Vec<String>` | — | **the runner's own rule** — an empty entry is a child-process construction hazard | `RunnerRequest::argv_entry_violation` |
| `root` | `PathBuf` | caller argument (not from the spec) | **the runner's own rule** — canonicalizes, must be a directory | `RunnerRequest::validate` |
| `cwd` | `Option<RelativePath>` | `CommandSpec::cwd` | **the runner's own rule** — canonicalized under `root`, must start with `root` and be a directory; `None` means `root` itself | `RunnerRequest::validate` |
| `environment` | `Vec<(String, String)>` | `CommandSpec::environment` (name/value pairs) | `eggwork-core` — `validate_environment_entries`: `<= MAX_ENV_COUNT` (256); name non-empty, `<= MAX_ENV_NAME_BYTES` (256), ASCII alphanumerics and underscore only, not starting with a digit, no `\0`; value `<= MAX_ENV_VALUE_BYTES` (16 KiB), no `\0`; no duplicate name | `RunnerRequest::validate` |
| `stdin` | `eggwork_core::StdinPolicy` | `CommandSpec::stdin` | `eggwork-core` — `StdinPolicy::validate` rejects `Bytes` over `MAX_STDIN_BYTES` (4 MiB) | `RunnerRequest::validate` |
| `timeout` | `Duration` | `Duration::from_millis(timeout_millis)` | `eggwork-core` — `validate_timeout`: non-zero and `<= MAX_TIMEOUT` (7 days) | `RunnerRequest::validate` |
| `capture_limit` | `usize` | `output.capture_limit_bytes` | `eggwork-core` — `OutputPolicy::validate`: `<= MAX_CAPTURE_BYTES` (16 MiB) | `RunnerRequest::validate` |
| `event_chunk_bytes` | `usize` | `output.event_chunk_bytes` | `eggwork-core` — `OutputPolicy::validate`: `1..=MAX_EVENT_CHUNK_BYTES` (64 KiB) | `RunnerRequest::validate` |
| `overflow` | `eggwork_core::OverflowPolicy` | `output.overflow` | `eggwork-core` — both `Truncate` and `Terminate` are valid | `RunnerRequest::validate` |
| `provenance` | `ExecutionProvenance` | caller argument | — | — |
| `sandbox` | `SandboxRequest` | `isolation` | — (profile checked in `ExecutionSetup::prepare` and `SandboxChannel::new`) | — |
| `resources` | `ResourceSetupRequest` | `resources.memory_bytes` / `.cpu_millis` / `.pids` | `eggwork-core` — `ResourceRequirements::validate` bounds each dimension; enforcement is decided by `ExecutionSetup` | `RunnerRequest::validate` |

`root` deserves emphasis: it is **not** derived from the spec. Authorization of the root is the caller's decision, passed alongside the spec. The runner's contribution is to canonicalize it and to make it the confinement boundary for `cwd`.

`RunnerRequest::new(argv, root)` supplies conservative defaults for an embedder that is not going through `from_spec`: `cwd: None`, empty environment, `StdinPolicy::Null`, `timeout: 30s`, `capture_limit: MAX_CAPTURE_BYTES`, `event_chunk_bytes: MAX_EVENT_CHUNK_BYTES`, `overflow: Truncate`, `SandboxRequest::None`, and all three resource `Requirement`s `NotRequested`. The defaults are convenience, not a bypass: the request they produce is covered by the same contract as one built from a spec, and `RunnerRequest::contract_error()` will say so before anything runs.

### `from_spec`

```rust
pub fn from_spec(
    spec: &ExecutionSpec,
    root: impl Into<PathBuf>,
    provenance: ExecutionProvenance,
) -> Result<Self, RunnerError>
```

It runs `spec.validate()` first and maps a `ValidationError` to `RunnerError::InvalidRequest(e)` — the typed error, not a rendered string, so the rule that refused the request stays identifiable. Only then does it clone and destructure `spec.command`, so a spec that is invalid at the domain layer never becomes a runner request. The mapping is otherwise mechanical, with two decisions worth naming:

- **Isolation becomes a profile string.** `IsolationRequirement::None → SandboxRequest::None`, `BestEffort → BestEffort { profile: WORKSPACE_RW_PROFILE }`, `Required → Required { profile: WORKSPACE_RW_PROFILE }`. `WORKSPACE_RW_PROFILE` (`"workspace_rw"`) is therefore the only profile the protocol can request; the runner independently rejects anything else as `"unsupported sandbox profile"`. The name is an exported constant rather than a literal, because the trusted helper is a standalone binary that keeps its own copy of the spelling — the helper's test pins the two together.
- **Everything the runner does not own is dropped.** The destructure binds `argv, cwd, environment, stdin, timeout_millis, output, resources, isolation, ..` — so `declared_outputs` and `network` are discarded by construction, not by later ignoring.

`RunnerRequest::validate` still runs at `run`, so `from_spec` validation is a fail-fast convenience layered on top of the enforcement point, not a replacement for it.

### One `StdinPolicy`

There is one `StdinPolicy` type, and it is core's: `eggwork_core::StdinPolicy`. It derives `Serialize`/`Deserialize`, has a public `validate() -> Result<(), ValidationError>`, is what an `ExecutionSpec` carries, and is what the runner's private `stdin` field holds.

The runner used to keep a second, crate-local enum with its own `Debug` and its own copy of the `MAX_STDIN_BYTES` bound, and converted between the two by hand in `from_spec`. That copy was a second owner of a rule core already owned, and it is gone. The public path `eggwork_runner::StdinPolicy` survives as a re-export of the core type, so an embedder that already imports it from the runner keeps compiling and gets the same type — the `Debug` impl is core's, which redacts the bytes and prints only their length, exactly as the local copy did.

The bound is now enforced once, by `eggwork_core::StdinPolicy::validate`, and the runner consumes it like every other core rule.

## ExecutionSetup — the platform seam

```rust
#[async_trait::async_trait]
pub trait ExecutionSetup: Send + Sync {
    async fn prepare(
        &self,
        sandbox: &SandboxRequest,
        resources: &ResourceSetupRequest,
        root: &Path,
    ) -> Result<SetupOutcome, RunnerError>;

    async fn execution_capabilities(&self) -> Vec<String> { Vec::new() }
    fn sandbox_helper_path(&self) -> Option<&Path> { None }
}
```

Only `prepare` is required. `execution_capabilities` and `sandbox_helper_path` are defaulted, so a minimal implementation cannot accidentally advertise a feature or a helper. The doc on `execution_capabilities` is normative: "Each returned feature string must reflect a live probe — never a compile-time or configuration-time guess." `LocalProcessRunner::execution_capabilities` forwards it and adds the constraint that callers "MUST combine these with the static protocol/workspace/artifact feature list; the runner owns dynamic facts only."

`prepare` is a *reporting* hook, not a command builder. It returns what enforcement will be attempted; the actual argv composition happens later in `run` via `SandboxChannel::command`, `direct_command`, and `wrap_resource_command`. Note that both in-tree implementations bind the `root` parameter as `_root` and never read it — the spec's `root` is re-derived from `request.root.canonicalize()` inside `SandboxChannel::new`. The parameter is currently a seam for future backends, not a used input.

### `TrustedLandlockSetup`

`TrustedLandlockSetup::new(helper_path)` takes an explicit path; `discover_sibling()` resolves `current_exe().canonicalize().parent()/eggwork-sandbox-helper`, falling back to the bare relative name `eggwork-sandbox-helper` if any step fails. `LocalProcessRunner::default()` uses `discover_sibling()` on Linux and `NoExecutionSetup` elsewhere.

`TrustedLandlockSetup::prepare` does three things, in this order:

1. **Probe OS resource controls.** If `resources.is_requested()`, call `SystemdCgroupBackend::probe`. Success yields `ResourceSetupOutcome::Applied { backend: "systemd-cgroup-v2" }`; failure yields `NotApplied { reason }`, or `Err(RunnerError::Setup(reason))` when `resources.has_required()`. No request yields `NotRequested`.
2. **Downgrade resources if the helper is untrusted.** If resources came back `Applied` *and* `verify_trusted_helper` fails, the resource outcome is downgraded to `NotApplied { reason }` (or the required case errors). This coupling is real: on Linux the helper is also the cgroup path (it self-applies `rlimit`s and reports limit breaches over the status socket), so advertising systemd limits while the helper is untrusted would be a claim the enforcement path cannot keep.
3. **Resolve the sandbox profile.** `None → NotRequested`. A profile other than `WORKSPACE_RW_PROFILE` → `NotApplied` for `BestEffort`, `Err(Setup)` for `Required`. Otherwise `verify_trusted_helper` decides: `Applied { profile }`, `NotApplied { reason }` for `BestEffort`, `Err(Setup)` for `Required`.

`TrustedLandlockSetup::execution_capabilities` is where capability advertisement is earned. It pushes `LANDLOCK_WORKSPACE_RW_CAPABILITY` (`"isolation.landlock.workspace-rw.v1"`) only when `verify_trusted_helper(...).is_ok() && probe_landlock_ruleset()`. `probe_landlock_ruleset` builds an in-process Landlock **ABI V4** ruleset with `AccessFs::from_all(abi)`, `create()`s it, and adds `PathBeneath` rules over `/usr`, `/etc/ld.so.cache`, `/etc/ssl/certs`, `/dev/null` with the *same* access masks the helper uses — directories get `from_read | Execute`, `/dev/null` gets `ReadFile | WriteFile`, other files get `ReadFile`. It deliberately never calls `restrict_self`, so the probe process is not confined; it proves the kernel supports the precise rights, nothing more. Every failure folds to `false`. It then runs three independent `SystemdCgroupBackend::probe` calls and, for each success, adds `CGROUPS_V2_MEMORY_CAPABILITY` (`resources.cgroups-v2.memory`), `CGROUPS_V2_CPU_CAPABILITY` (`resources.cgroups-v2.cpu`), or `CGROUPS_V2_PIDS_CAPABILITY` (`resources.cgroups-v2.pids`).

**Every feature name this crate advertises is an exported constant.** `LANDLOCK_WORKSPACE_RW_CAPABILITY`, the three `CGROUPS_V2_*_CAPABILITY` names, and `WORKSPACE_RW_PROFILE` are the crate's public surface for those bytes; the advertisement site names the constants and no literal. The node's server consumes the same constants, so the producer of a feature name and its consumer cannot drift apart — a name that differed by one character would turn every required resource dimension into a `capability_mismatch` refusal. The values themselves are frozen and pinned by `capability_feature_string_is_frozen_for_workspace_rw_profile`, `cgroup_capability_feature_strings_are_frozen`, and `the_workspace_rw_profile_name_is_frozen`.

**How the helper argv is composed.** `SandboxChannel::new` decides whether the helper is used at all. It accepts a profile when the request is `BestEffort`/`Required` with `WORKSPACE_RW_PROFILE`, or accepts `None` when `SandboxRequest::None` coincides with a resource request (helper-for-resources-only); anything else is `Err(Setup("unsupported sandbox profile"))`. Then:

- A `tempfile::Builder::new().prefix("eggwork-sandbox-").tempdir()` is created and chmod-ed to `0700`.
- `spec.json` and `status.sock` are named inside it; a `tokio::net::UnixListener` is bound to `status.sock`. **The runner is the socket server and the helper connects to it** — the target never has the socket.
- The launch environment is rebuilt from scratch by the same `build_child_environment` the direct path uses: the node's baseline (`PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `LC_ALL=C.UTF-8`, `CI=1`, `NO_COLOR=1`, `TERM=dumb`, `GIT_TERMINAL_PROMPT=0`, `PAGER=cat`), then the request's environment with denied keys dropped and node-owned keys replaced by the node's values, then `EGGWORK_EXECUTION_ID` and `EGGWORK_EXECUTION_GENERATION` when provenance carries them. Sharing the builder is what stops the two launch paths from disagreeing.
- `SandboxLaunchSpec { schema_version: 1, profile, root, cwd, argv, environment, resource_memory_bytes, resource_cpu_millis, resource_pids }` is serialized with `serde_json` and rejected above 1 MiB. The three resource fields are populated only when `resources_applied`, using `.then_some(...).flatten()` so a `NotRequested` dimension stays `None`.
- The spec is written with `OpenOptions::new().write(true).create_new(true).mode(0o600)` — create-new, so a pre-existing file at that path cannot be followed.

`SandboxChannel::command` is then the actual child command: `Command::new(helper_path) --spec <spec_path> --status <status_path>`, `current_dir` set to the spec's parent, all three stdio handles piped, `kill_on_drop(true)`, `env_clear()` followed by the node's own baseline. The helper must not inherit the daemon's `LD_*`, `GIT_*` or cloud variables while it is parsing a specification that a deny-list was supposed to keep clean. The environment the *target* will see is not the helper's environment — it is the `environment` array inside the spec, which the helper applies itself after `env_clear()`.

`SandboxChannel::wait` is the handshake, and it is the reason required isolation cannot be spoofed by target output. It selects on cancellation or `timeout(5s, listener.accept())`, then reads one status byte with a 5 s timeout:

| Status byte | Runner interpretation |
| --- | --- |
| `0`, `2`, `3`, `30`, `31` | `Ok(None)` — the helper never reached a confined-ready state (the helper emits `2` for unsatisfiable resource limits or a ruleset failure, `30` for `RulesetStatus != FullyEnforced`, `31` for missing `no_new_privs`; `3` is currently unused) |
| `>= 4` | `BeforeTarget(RunnerError::Setup("sandbox helper rejected its launch specification (status N)"))` |
| `1` | Confined and about to spawn: read a second byte. `1` → `Ok(Some(SandboxSession))`; `0` → `AfterTarget(SunnerError::Spawn("sandbox target could not start"))`; anything else → `AfterTarget(Setup("sandbox status was invalid"))` |
| other | `BeforeTarget(Setup("sandbox status was invalid"))` |

Cancellation is checked before the accept and before each status read, so a cancel arriving during the handshake cannot produce a target. The third status byte is read later, after the child exits, by `SandboxSession::resource_limits_exceeded` with a 2 s timeout: bit 1 → `ResourceDimension::Memory`, bit 2 → `ResourceDimension::Pids`.

**What `LANDLOCK_WORKSPACE_RW_CAPABILITY` means.** The constant is the frozen public feature name for the *closed* `workspace_rw` Landlock profile used by both `IsolationRequirement::BestEffort` and `IsolationRequirement::Required`. It is not a version of Landlock itself and not a claim about rlimits — it is a single feature identifier pinned so that a node's advertised feature list and the runner's enforcement decision refer to the same contract. It is asserted literally in `capability_feature_string_is_frozen_for_workspace_rw_profile` so a rename cannot silently break wire compatibility, and it is the same constant the server's `check_isolation_supported` compares against — the runner produces it, the server consumes it, and neither re-types it.

### `NoExecutionSetup`

`NoExecutionSetup` is a unit struct (`#[derive(Debug, Default)]`) meaning "no platform backend is configured". Its `prepare` fails closed for anything required:

- `SandboxRequest::Required { .. }` or `resources.has_required()` → `Err(RunnerError::RequiredSetupUnavailable)`.
- `SandboxRequest::None` → `SandboxOutcome::NotRequested`.
- `BestEffort`/`Required` otherwise → `SandboxOutcome::NotApplied { reason: "no sandbox backend is configured" }` (the `Required` arm is unreachable after the guard, but the match is written exhaustively).
- Resources → `NotApplied { reason: "no resource-control backend is configured" }` when requested, `NotRequested` otherwise.

It does not override `execution_capabilities`, so it inherits the default `Vec::new()`. That is the honest answer: a node with no backend advertises no dynamic execution features at all, rather than advertising a compile-time guess.

### Result types

`SetupOutcome { sandbox: SandboxOutcome, resources: ResourceSetupOutcome }` is what `prepare` returns.

`SandboxOutcome` has four states: `NotRequested`, `NotApplied { reason }`, `Applied { profile }`, `Failed { reason }`. `Failed` is declared and mapped in `execution_result` but is not constructed by either in-tree implementation — best-effort degradation always produces `NotApplied` with a reason, and required failures become `Err`. The distinction is preserved so a future backend can distinguish "not attempted" from "attempted and failed".

`ResourceSetupOutcome` is structurally identical: `NotRequested`, `NotApplied { reason }`, `Applied { backend }`, `Failed { reason }`.

Both are `Debug + Clone + PartialEq + Eq`, which is what lets `unsupported_resource_limits_are_reported_or_rejected_before_spawn` pattern-match on them directly.

`ResourceSetupRequest` carries three `eggwork_core::Requirement<T>` dimensions and has three private helpers: `is_requested()` (any non-`NotRequested`), `has_required()` (any `Required`), and `properties()`, which renders systemd properties — `MemoryMax=<bytes>` plus `MemorySwapMax=0` for memory, `CPUQuota=<value/10>.<value%10>%` for CPU (millis to a systemd percentage), `TasksMax=<value>` for pids.

### `#[cfg(target_os = "linux")]` gating and fail-closed fallbacks

Landlock and systemd are Linux kernel interfaces, so the crate gates rather than abstracts them:

| Linux-only item | Non-Linux fallback |
| --- | --- |
| `SystemdCgroupBackend` (real: `discover`, `probe`, `base_command`, `wrap`, `unit_result`, `stop_unit`, `systemctl_path`) | unit struct with only `async fn probe` returning `Err("OS resource controls are unsupported on this platform")` |
| `resource_unit_name()` | absent (only called from Linux code) |
| `probe_landlock_ruleset()` | `fn probe_landlock_ruleset() -> bool { false }` |
| `verify_trusted_helper(path)` | `Err("trusted Landlock is unsupported on this platform")` |
| `SandboxChannel`, `SandboxSession`, `SandboxLaunchSpec`, `SandboxWaitError`, `wrap_resource_command` | absent; `run` sets `channel: Option<()> = None`, `resource_unit: Option<String> = None`, `resource_limits_exceeded: Vec<..> = Vec::new()`, `resource_cleanup_warning: Option<String> = None` |

Every fallback is a refusal or an empty result. None of them is a permissive default, so a non-Linux host cannot accidentally advertise or claim an enforcement capability.

## Helper trust boundary

`verify_trusted_helper` is the entire Linux helper trust policy, and it is one function because a second, independently drifting copy is itself a defect. The doc comment explains the failure mode it prevents: "A duplicated stricter copy denies a user-owned helper that this function would accept, which makes an unprivileged user-scope installation permanently unable to advertise required filesystem isolation."

**Exact checks, in order:**

1. `symlink_metadata(path)`; any error → `Err("sandbox helper is unavailable")`. `symlink_metadata` (not `metadata`) is deliberate — it does not follow the final symlink.
2. The helper itself must satisfy all of: `is_file()`, `!file_type().is_symlink()`, `uid() == 0 || uid() == geteuid()`, `mode & 0o022 == 0` (neither group- nor world-writable), `mode & 0o111 != 0` (executable by someone). Any failure → `Err("sandbox helper trust check failed")`.
3. The parent directory: `path.parent()` must exist, canonicalize (`Err("sandbox helper location is invalid")` otherwise), and then be a directory owned by root or the effective user with `mode & 0o022 == 0`. Failure → `Err("sandbox helper directory trust check failed")`.
4. Every remaining ancestor, walking up from the canonicalized parent to the filesystem root: each must be a directory, owned by root or the effective user, and not group/world-writable — **unless** it is a `sticky_root_directory`, defined as `is_dir() && uid() == 0 && mode & 0o1000 != 0`. The sticky-bit exemption exists so `/tmp` (mode `1777`, world-writable) does not permanently break a user-scope installation. Failure → `Err("sandbox helper ancestor trust check failed")`.
5. Any `metadata()` error along the ancestor walk → `Err("sandbox helper ancestor is invalid")`.

The non-Linux variant is unconditional refusal: `Err("trusted Landlock is unsupported on this platform")`. It does not inspect the path at all, so there is no platform on which this function returns `Ok` without a Landlock-capable kernel.

**The single-delegation invariant.** Within the crate, all three call sites in `TrustedLandlockSetup` go through this one function: the resource downgrade and the sandbox resolution in `TrustedLandlockSetup::prepare`, and the capability advertisement in `TrustedLandlockSetup::execution_capabilities`. Outside the crate, the server's `operations::trusted_helper` is a one-line delegation — `eggwork_runner::verify_trusted_helper(path).is_ok()` in the server's `operations::trusted_helper` — used by `doctor` so that diagnostics cannot report an answer the enforcement path would refuse. This is what makes "advisory and enforcement can never disagree" a structural property rather than a convention: there is no second implementation to drift.

`SystemdCgroupBackend` applies an analogous (but independent, and *stricter*) root-only trust pattern to its own binaries: `discover()` requires `/usr/bin/systemd-run` or `/bin/systemd-run` to canonicalize, be a regular file, have `uid() == 0`, have `mode & 0o022 == 0`, and be executable; `systemctl_path()` repeats the same four checks for `/usr/bin/systemctl` or `/bin/systemctl`. These are not the helper trust boundary and are not delegated to `verify_trusted_helper` — the helper is user-scope-installable, `systemd-run` is not.

## Process lifecycle

`LocalProcessRunner::run` is a single `async fn` on `&self` with three arguments: `RunnerRequest`, `CancellationToken`, `mpsc::Sender<OutputChunk>`. The sequence:

**1. Cancellation gate.** `cancellation.is_cancelled()` → `Err(RunnerError::CancelledBeforeSpawn)`. Nothing has been touched.

**2. Authorized root enforcement.** `let cwd = request.validate()?`. This canonicalizes `root` (requiring a directory), canonicalizes `root.join(cwd)` when a `RelativePath` is present (requiring containment and a directory), and returns the working directory to use. Every `RunnerError::InvalidRequest` / `InvalidRoot` / `InvalidWorkingDirectory` originates here.

**3. Platform gate.** `#[cfg(not(unix))]` returns `Err(RunnerError::UnsupportedPlatform)` immediately after validation — before `prepare`, before any helper, before `spawn`. The `let _ = (cwd, request, output_tx);` in that arm is there to keep the unused bindings from becoming warnings on that configuration.

**4. Setup.** `self.setup.prepare(&request.sandbox, &request.resources, &request.root).await?`. The `?` means a required-sandbox or required-resource refusal is an `Err`, never a silent downgrade to direct execution.

**4a. Degradation diagnostics.** `report_setup_degradations` turns the outcomes `prepare` returned into diagnostic payloads and sends them on the output channel. It runs here — after the setup resolved and before any process exists — so the diagnostic reaches the caller *before* the target can produce a single byte of output. See [Reporting a degradation](#reporting-a-degradation) below.

**5. Helper selection (Linux).** `use_helper` is true when (`sandbox != None` **and** `setup.sandbox` is `Applied`) or `setup.resources` is `Applied`, **and** `sandbox_helper_path()` is `Some`. The two conditions are the only way the helper subprocess is created.

**6. Channel construction (Linux, if `use_helper`).** `SandboxChannel::new` may fail. On failure, if the request is not `Required` and has no required resources, both outcomes are downgraded to `NotApplied { reason }`, the degradation is reported on the output channel, and execution proceeds **without** the helper. Otherwise the error propagates.

**7. Command assembly.** `channel.command()` or `direct_command(&request, cwd)`, then `wrap_resource_command(command, &request.resources, &setup.resources)` which — only when the outcome is `Applied { backend: "systemd-cgroup-v2" }` — rewrites the command through `SystemdCgroupBackend::wrap` into `systemd-run [--user] --scope --quiet --unit=eggwork-<pid>-<nanos>.scope --working-directory <cwd> --property=… -- <program> <args…>`. When both isolation and resources apply, the systemd scope wraps the *helper*, and the helper execs the target inside the scope. The wrapper's own environment is `env_clear()` plus `DBUS_SESSION_BUS_ADDRESS`, `XDG_RUNTIME_DIR`, `HOME` (only if present in the parent), `PATH=/usr/bin:/bin`, and the wrapped command's explicit environment. Non-Linux-unix skips this and calls `direct_command` directly.

**8. Process group and spawn.** `command.process_group(0)` — the child becomes its own process-group leader — then `command.spawn().map_err(|e| RunnerError::Spawn(e.to_string()))`. Every `Command` built in this crate sets `.kill_on_drop(true)`.

**9. Handshake (Linux + channel).** `SandboxChannel::wait(&mut child, cancellation.clone())` has four outcomes:

- `Ok(Some(session))` — the target is confirmed confined and started; the session is retained for post-exit limit attribution.
- `Ok(None)` and the request is not required — the helper child is `start_kill`ed and waited, the outcomes are downgraded (`"Landlock helper could not enforce the requested profile"` / `"resource helper could not verify the cgroup limits"`), the degradation is reported on the output channel, and the **command is rebuilt and respawned** without the helper. The diagnostic is emitted *after* the handshake failure is classified and *before* the respawn: the target is about to run with no Landlock ruleset and no cgroup verification, and the caller has to learn that before the target runs.
- `Ok(None)` and the request is required — kill, wait, `Err(RunnerError::RequiredSetupUnavailable)`.
- `Err(_)` — kill, wait, return the error. `BeforeTarget` errors are recoverable (respawn unwrapped) when nothing was required; `AfterTarget` errors are always terminal. A recoverable `BeforeTarget` error reports the degradation before the respawn, exactly as the `Ok(None)` arm does.

Note the ordering: `let process_group = child.id();` is taken *after* the handshake, so the group that gets signalled is the group of whichever child actually ended up running.

**10. Stdin.** For `StdinPolicy::Bytes(bytes)`, a `tokio::spawn`ed task does `stdin.write_all(&bytes).await` then `drop(stdin)` to close the pipe. For `StdinPolicy::Null` the pipe is dropped immediately (child sees EOF). The `Bytes` arm also has a defensive third case (`StdinPolicy::Bytes(_)` with no piped handle) that drops the handle, so a child that never reads stdin cannot wedge on a full pipe.

**11. Readers.** `stdout` and `stderr` are `take()`n from the child (absence → `RunnerError::Io("stdout pipe missing")` / `"stderr pipe missing"`), and each is handed to `spawn_reader`.

**12. Output monitor.** `let (overflow_tx, mut overflow_rx) = watch::channel(false);` — a single watcher shared by both readers so that either stream crossing the capture limit can terminate the execution.

**13. The wait race**, `tokio::select! { biased; … }` in this precedence order:

```rust
_ = cancellation.cancelled()      => Some((Cancelled, None, None)),
_ = sleep(deadline)                => Some((TimedOut,  None, None)),
_ = overflow_rx.changed()          => monitor_termination(*overflow_rx.borrow()).map(|t| (t, None, None)),
result = child.wait()              => Some((Exited, status?, error?)),
```

`biased` is intentional and the ordering is a policy: cancellation outranks timeout, which outranks overflow, which outranks natural exit. A `None` result (the monitor settled *without* overflow — which can only happen after both readers hit EOF, which can only happen after the child exited) falls through to a second `child.wait()` after the `select!` so the real exit status is recovered instead of being discarded. The in-source comment above the `select!` explains the code shape too: the `child.wait()` continuation lives *after* the `select!` rather than inside an arm because awaiting it inside an arm makes the future `!Send`, and this `select!` runs inside a spawned task.

`monitor_termination(bool) -> Option<TerminationReason>` is the whole distinction in one line: `overflowed.then_some(TerminationReason::OutputLimit)`. `Some` is a termination; `None` is not. The in-source rationale is that the settled-but-not-overflowed case used to be reported as `Exited` with a `None` status, surfacing as `state: Failed, failure: Internal, exit_code: null` — a shape that reads as a product failure and is not one.

**14. Resource attribution (Linux).** Only when `termination == Exited`: ask the session for its status byte (`Memory`/`Pids` bits), then independently re-discover the backend and check `systemctl show <unit> --property=Result --value` (1 s timeout) for `"oom-kill"`, which adds `Memory` if not already present. Two independent sources for the same fact, because a cgroup OOM kill and a helper-reported rlimit breach are not the same signal.

**15. Resource cleanup (Linux).** `systemctl stop <unit>` (2 s timeout) when a unit exists; its error seeds `CleanupDiagnostics::process_group_signal_error`.

**16. Process-tree convergence.** The shared lifecycle calls two platform methods and never names a signal: `ProcessTree::converge_after_leader_exit()` when the leader already reported a status, and `ProcessTree::terminate_and_reap(TERMINATION_GRACE)` when it did not (timeout / cancel / overflow). Both return the same `TreeConvergence { status, signal_error, wait_error }`, which maps one-to-one onto `CleanupDiagnostics`; `run` never learns which backend it is talking to. Converging never revokes a status the leader already reported — that would rewrite a successful execution into `Failed`/`Internal`.

- **Unix** (`process_tree::imp`, `cfg(unix)`) — the leader was spawned with `process_group(0)` and the group id was captured at spawn, because `Child::id` returns `None` once the leader is reaped and the group is still needed after that.
  - *Status already known*: `terminate_group(group)`. `Ok(true)` (a live group was signalled) → `sleep(TERMINATION_GRACE)` → `kill_group`. `Ok(false)` (`ESRCH`, nothing left) → no-op. `Err` → recorded as `process_group_signal_error`.
  - *Status unknown*: `terminate_group`, grace sleep if signalled, then `kill_group` unconditionally (`ESRCH` tolerated inside `kill_group`), then `timeout(TERMINATION_GRACE, child.wait())`. Success yields the status; a `wait()` error sets `cleanup.wait_error` and leaves `status` `None`; a timeout sets `cleanup.wait_error = "child did not exit after SIGKILL"`.
- **Windows** (`process_tree::imp`, `cfg(windows)`) — the tree owner is a Job Object that already owns the leader and every descendant, so both branches are one `TerminateJobObject` call (`ChildWrapper::start_kill`) with no escalation window and nothing to wait for. `status` is always `None`: a job termination exit code describes Eggwork's kill, not the target's outcome, so reporting it as `exit_code` would be a fabricated fact — and Unix already reports `None` there, because a signalled process has no exit code.

  What Windows convergence does **not** claim: per-descendant exit observation. No safe-Rust surface in the reviewed dependency exposes a job-membership query, and adding one would mean Eggwork-owned `unsafe` Win32 FFI, which the workspace denies. Instead the invariant rests on three facts the shared lifecycle already provides: the Job Object stays owned for the whole execution, so nothing can escape the tree by outliving a leader; after `TerminateJobObject` returns the kernel has suspended and marked every member terminating, so no member can execute further user code; and `join_reader` still runs after convergence, and a pipe only reaches EOF once every process holding it has exited. Closing the Job Object handle is the backstop — the job is created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and the handle closes when the tree owner drops, which happens before `run` returns and therefore before the caller's execution permit is released.

**17. Readers joined.** `join_reader` on both handles; a `JoinError` or reader `RunnerError::Io` becomes `Err` from `run` (note: this discards whatever was captured — see Review focus).

**18. Result assembly.** `RunnerResult` is built with `exit_code: status.and_then(|s| s.code())`, the two captures, `cleanup`, `setup`, `request.resources`, `resource_limits_exceeded`, `stream_chunks_dropped: dropped_out + dropped_err`, and `request.provenance`.

### Environment clearing and the conservative baseline

The runner builds a child's environment in exactly one place — `build_child_environment` — and **both** of its launch paths use it: `direct_command` for the unconfined path, and `SandboxChannel::new` for the launch specification the sandbox helper reads. That is the whole point of the function: the two paths cannot disagree about what a child sees.

`direct_command` never inherits the caller's environment. `command.env_clear()` is unconditional, then the environment is assembled in three steps.

**1. The node's baseline goes in first.** `baseline_child_environment()` returns the node-owned baseline: on Unix `PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `LC_ALL=C.UTF-8`, `CI=1`, `NO_COLOR=1`, `TERM=dumb`, `GIT_TERMINAL_PROMPT=0`, `PAGER=cat`; on Windows `CI=1`, `NO_COLOR=1`, `TERM=dumb`, `GIT_TERMINAL_PROMPT=0`, `PAGER=cat`, plus `SystemRoot` and `windir` (from the parent, defaulting to `C:\Windows`) and `PATH=<SystemRoot>\System32;<SystemRoot>`. The Unix baseline is the shared environment policy's, read from the generated `environment_policy` module rather than written out here, so the runner and the sandbox helper cannot disagree about it. The Windows baseline is runner-local, because its values come from the parent process at spawn time and the sandbox helper is Linux-only so it never sees a Windows launch.

**2. The request's environment, under the shared policy.** Each entry is classified by the generated `environment_policy` module, whose single owner is `crates/eggwork-runner/policy/environment-policy.toml`:

- **Denied** — the caller may not set this key at all. The runner **drops** it, silently. This is the *request-normalisation boundary*: a hardened environment has to be a property of the node rather than something the caller has to get right, and the caller is not told. The list is matched case-insensitively by exact name or prefix: `LD_`, `DYLD_`, `PATH`, `IFS`, `SHELLOPTS`, `CDPATH`, `BASH_ENV`, `ENV`, `PYTHONPATH`, `PYTHONHOME`, `NODE_OPTIONS`, `RUBYOPT`, `PERL5OPT`, `GIT_ASKPASS`, `SSH_ASKPASS`, `GIT_CONFIG`, `GIT_CONFIG_`, `AWS_`, `AZURE_`, `GOOGLE_`, `SSH_AUTH_SOCK`.
- **Node-owned (reserved)** — the node sets this key itself, so the caller's value is **discarded in favour of the node's**. The node-owned class is the node's own baseline, and the baseline is platform-shaped: on Unix it is the policy's `RESERVED_KEYS`, and on Windows it is the runner-local baseline's keys. The match is case-insensitive, so a caller's `lang` is the same key as the node's `LANG`.
- **Caller-controlled** — everything else passes through. A repeated key is resolved to its **last** value, so the launch specification can never carry a duplicate key.

This is a *deny* filter over a baseline, not an allow list — but combined with `env_clear()` the effective result is allow-shaped: only the node's baseline plus explicitly requested, non-denied keys reach the child.

**3. Provenance last.** `EGGWORK_EXECUTION_ID` (from `ExecutionId::as_str`) and `EGGWORK_EXECUTION_GENERATION` (from `ExecutionGeneration::get()`) when present. This is the documented "propagate neutral execution provenance through environment only where safe" of plan 002 §3.12 — the IDs are opaque strings with no routing authority — and they are appended after the caller's entries so a caller cannot forge them.

**The reserved-key rule is what closes the collision hole.** Before it, the launch-specification builder started from the fixed baseline and then *appended* every non-denied caller key, and none of the baseline keys except `PATH` was denied. A caller that set `LANG` therefore produced a specification with **two** `LANG` entries; the helper refuses a specification with duplicate keys; the runner read that refusal as a pre-target setup failure and — for a **best-effort** request only — rewrote the sandbox outcome to `NotApplied` and re-spawned the target directly with no Landlock ruleset at all. The observable result was a successful, entirely unconfined execution whose only record of the degradation was a `NotApplied` in the terminal snapshot. The `Required` path was unaffected and still failed closed.

**The `PATH` contract.** `PATH == /usr/bin:/bin` is an enforced contract on both sides: the runner guarantees the value in the child environment and in the launch specification, and the sandbox helper refuses a specification whose `PATH` differs. It is a **Linux/Unix** contract — the Windows baseline is explicitly outside it, so the Windows `PATH` must not be "fixed" to the Unix value.

The `#[cfg(windows)]` branch was originally unreachable: `run` refused on non-Unix before a command was ever built, so the shaping could not and did not fix Windows qualification on its own. Foundation M004 removed that refusal, which makes the branch load-bearing — `SystemRoot` is what the Windows loader needs, and `PATH` is what resolves `cmd.exe`. `windows_process_tree.rs::environment_is_the_documented_baseline_and_leaks_nothing` asserts both the baseline contents — taken from the runner's own `baseline_environment_keys()`, not from a second hand-written list — and that no ambient daemon variable reaches the target.

### Bounded capture: head and tail

`BoundedCapture` retains `head: Vec<u8>`, `tail: VecDeque<u8>`, `total_bytes: u64`, `omitted_bytes: u64`. `BoundedCapture::push(bytes, limit)`:

```rust
let head_limit = limit.div_ceil(2);
let tail_limit = limit.saturating_sub(head_limit);
self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
// per byte: fill head until head_limit; else if tail_limit > 0, evict front when full and push back
self.omitted_bytes = self.total_bytes.saturating_sub((self.head.len() + self.tail.len()) as u64);
```

Consequences worth checking as a reviewer: retention is exactly `limit` bytes for `limit >= 1` (`div_ceil` gives the extra byte to the head on odd limits, so a `limit` of 1 keeps one head byte and no tail); for `limit == 0` nothing is retained and `omitted_bytes == total_bytes`; `total_bytes` and `omitted_bytes` are `saturating`, so they cannot underflow or wrap on adversarial output. The per-byte loop means a single large `push` fills the head, then streams the remainder through the `VecDeque` eviction — the head and tail of a huge burst come from the same buffer, and the tail is always the *last* `tail_limit` bytes seen. `output_capture_retains_head_tail_with_totals` pins the semantics with `push(b"abcdefghij", 6)` → head `abc`, tail `hij`, omitted `4`.

### Streaming: dropping chunks while draining continues

`spawn_reader` is the drain loop. Per read of at most `READ_BUFFER.min(chunk_limit)` bytes (8192 cap):

1. `capture.push(&buffer[..count], capture_limit)` — always, before anything else.
2. If `overflow == Terminate` and `total_bytes` crossed `capture_limit` on this read (`before <= capture_limit && after > capture_limit`), `overflow_tx.send(true)`.
3. `for bytes in buffer[..count].chunks(chunk_limit)` → `output_tx.try_send(OutputChunk::Stdout(bytes))` or `OutputChunk::Stderr(bytes)` depending on the reader's stream; on **any** error, `dropped += bytes.len() as u64`.

Why this is the right trade: the reader's job is to keep the pipe empty, because a child blocked on a full stdout pipe can never reach its own exit, never observe cancellation, and never respond to a timeout. `try_send` therefore never awaits. A full or closed channel costs *event* fidelity, not process progress, and the loss is machine-reported through `stream_chunks_dropped` so the caller can decide whether a missing chunk is acceptable. The alternatives are both worse: blocking on `send` reintroduces the deadlock the drain exists to prevent, and dropping the read (or the child) turns a slow consumer into a correctness failure. The `BoundedCapture` totals remain exact regardless of channel pressure, so the durable record of what the child produced is not a function of consumer speed.

## Termination and cleanup

`TERMINATION_GRACE = Duration::from_millis(300)` is the single grace constant, used both as the `SIGTERM → SIGKILL` delay and as the bound on the post-`SIGKILL` `child.wait()`.

| Trigger | `TerminationReason` | Mechanism |
| --- | --- | --- |
| deadline elapsed (`sleep(deadline)`) | `TimedOut` | `killpg(SIGTERM)` → 300 ms → `killpg(SIGKILL)` → `wait` bounded at 300 ms |
| `CancellationToken::cancelled()` after spawn | `Cancelled` | identical to timeout |
| capture limit crossed with `OverflowPolicy::Terminate` | `OutputLimit` | identical to timeout |
| leader exited, descendants remain | `Exited` (status preserved) | `killpg(SIGTERM)` → 300 ms → `killpg(SIGKILL)`; the leader's `ExitStatus` is carried through untouched |
| cancellation before any child | *(no result)* | `Err(RunnerError::CancelledBeforeSpawn)` |
| cancellation during the sandbox handshake | *(no result)* | `Err(SandboxWaitError::{BeforeTarget, AfterTarget}(CancelledBeforeSpawn))` → `RunnerError::CancelledBeforeSpawn` |
| leader exited normally, no descendants | `Exited` | `killpg` returns `ESRCH` → `Ok(false)` → immediate return, no grace sleep |

**Signalling.** `terminate_group` and `kill_group` are `#[cfg(unix)]` and both use `nix::sys::signal::killpg` on the pid captured from `child.id()`. `terminate_group` maps `ESRCH` to `Ok(false)` (nothing to signal) and any other errno to `Err(String)`; `kill_group` maps `ESRCH` to success and propagates other errnos. The group works because `command.process_group(0)` makes the direct child the group leader, so `killpg` reaches every descendant that stayed in the group.

**Grace, force, reap.** `SIGTERM` gives cooperative children a chance to flush and exit; `SIGKILL` after 300 ms bounds the wait; `timeout(TERMINATION_GRACE, child.wait())` bounds the reap so a pathological child cannot hold the caller's future. The leader is always reaped; descendants are killed but reaped by init/the subreaper, which is why the descendant tests assert `/proc/<pid>/stat` is absent or state `Z`.

**The rule that cleanup never rewrites a known exit outcome.** In the `Some(status)` branch the group is cleaned *after* the status is known and the status is returned unchanged. The known reason survives every cleanup failure, and the only representation of that failure is `CleanupDiagnostics`. `CleanupDiagnostics` has three independent fields — `process_group_signal_error`, `wait_error`, `stdin_error` — and `execution_result` folds them into a single `cleanup_warning` by priority (`process_group_signal_error` → `wait_error` → `stdin_error`) while `state`, `exit_code`, and `failure` are computed solely from `termination`, `exit_code`, and `resource_limits_exceeded`. `cleanup_warning_does_not_rewrite_process_outcome` pins this: an `Exited`/`Some(9)` result with a `process_group_signal_error` still reports `Failed` with `exit_code: Some(9)` and the warning attached. This is the crate-side implementation of ADR-0003's "Terminal state is single-assignment. Cleanup diagnostics may be appended without changing the chosen terminal outcome."

## Result and error model

`RunnerResult`:

| Field | Meaning |
| --- | --- |
| `termination` | `Exited` / `TimedOut` / `Cancelled` / `OutputLimit` |
| `exit_code` | `Some(code)` when the leader produced one; `None` for signal-terminated leaders and failed waits |
| `stdout`, `stderr` | `BoundedCapture` with exact `total_bytes` / `omitted_bytes` |
| `cleanup` | `CleanupDiagnostics`, three optional strings |
| `setup` | `SetupOutcome` as it actually resolved, after any downgrade |
| `resource_request` | the requested dimensions, echoed so a caller can pair request with outcome |
| `resource_limits_exceeded` | `Vec<ResourceDimension>` from the helper status byte and `Result=oom-kill` |
| `stream_chunks_dropped` | `dropped_out + dropped_err` |
| `provenance` | echoed back verbatim |

`ExecutionProvenance { execution_id: Option<ExecutionId>, generation: Option<ExecutionGeneration> }` is the round-trip handle: the caller supplies it, the runner injects it into the child environment, and returns it on the result so a caller can attribute a receipt without re-deriving anything.

`OutputChunk` is the streaming unit, and it is an enum: `OutputChunk::Stdout(Vec<u8>)`, `OutputChunk::Stderr(Vec<u8>)` and `OutputChunk::Diagnostic(String)`. Its doc says consumers "must treat bytes as arbitrary binary data" — there is no encoding assumption anywhere in the reader. The `Diagnostic` variant carries a bounded payload (see [protocol.md](protocol.md#the-execution-diagnostic-payload-schema-version-1)) rather than bytes, and it is the only way a degradation reaches a caller.

### Reporting a degradation

A degradation is a *best-effort* request that could not be honoured. It is
reported on the output channel as `OutputChunk::Diagnostic`, and the server
forwards it as `ExecutionEventKind::Diagnostic`. The terminal snapshot
(`ExecutionResult.sandbox`, `ExecutionResult.resources`) carries the same fact;
the diagnostic exists so that a caller watching the stream sees the
degradation *while the execution is running* rather than only in the terminal
result.

**Why the runner and not the server.** The server learns the outcome from
`RunnerResult`, which arrives after the execution has finished. A diagnostic
the server derived from that result would necessarily arrive after all target
output, which is the gap this reporting exists to close. The runner knows
*why* an enforcement was not applied at the moment it decides not to apply
it, and the output channel is the one carrier that preserves the order
between the runner's knowledge and the target's output.

**Where the runner emits.**

| Site | When | Ordering |
| --- | --- | --- |
| `report_setup_degradations` after `prepare` | unsupported profile, untrusted helper, no backend configured, resource-probe failure | before the target process exists |
| the `SandboxChannel::new` failure arm | the helper could not even be launched | before the target process exists |
| the `SandboxChannel::wait` recovery arms (`Ok(None)` and a recoverable `BeforeTarget` error) | the handshake failed and the target is about to be respawned directly | after the failure is classified, before the respawn |
| the post-mortem resource-observation block | the helper could not open a counter it had installed, so the limit cannot be *verified* | after the target exited, before the terminal event |

The last row is the one exception to "before the target's output", and it is
a property of the fact rather than of the mechanism: the helper reports an
unobservable counter only after the target has exited, so no implementation
could report it earlier. It still precedes the terminal event, because the
server drains the output channel before it publishes the terminal state.

**The rules.**

* A `Required` request is *refused*, never degraded. `prepare` returns an
  `Err` before any diagnostic could be emitted, and a refusal is not a
  degradation: reporting one would dress a fail-closed refusal up as an
  advisory.
* One degraded dimension produces exactly one diagnostic. `ReportedDegradations`
  keeps the bookkeeping, because a degradation can be discovered twice — a
  sandbox `prepare` could not apply may also be the sandbox the handshake
  could not enforce — and the second discovery refines the recorded outcome
  rather than duplicating the event.
* `resources.degraded` carries the affected `dimension`, and one event is
  emitted per affected dimension when more than one is affected.
* An execution with nothing degraded emits no diagnostic at all.
* The runner never emits a diagnostic for a degradation it did not classify
  itself: the payload and the terminal snapshot are derived from the same
  `SetupOutcome`, so the two cannot disagree.

`RunnerResult::execution_result()` — also reachable via `impl From<RunnerResult> for ExecutionResult` — maps to the protocol-neutral result:

| Condition | `ExecutionState` | `ExecutionFailure` |
| --- | --- | --- |
| any `resource_limits_exceeded` | `Failed` | `ResourceLimit` |
| `Exited` + `exit_code == Some(0)` | `Succeeded` | `None` |
| `Exited` + any other code | `Failed` | `None` |
| `Exited` + `exit_code == None` | `Failed` | `Internal` |
| `TimedOut` | `TimedOut` | `None` |
| `Cancelled` | `Cancelled` | `None` |
| `OutputLimit` | `Failed` | `OutputLimit` |

`resource_limits_exceeded` outranks termination: a run that was killed by its own memory limit is `ResourceLimit`, not the incidental `TimedOut`/`Exited` that came with it. `Exited` with `exit_code: None` is the `Internal` shape — it covers both a leader that died from a signal and a `child.wait()` that returned an error (the latter also carrying `cleanup.wait_error`). `RunnerResult::resource_result` then reports each resource dimension independently as `NotRequested` / `NotApplied { reason }` / `Applied { backend }` / `LimitExceeded { backend }`, so a partially satisfied resource request is visible per dimension rather than collapsed.

`RunnerError` is the refusal/failure channel, with these exact messages:

| Variant | Meaning |
| --- | --- |
| `InvalidRequest(eggwork_core::ValidationError)` | argv, environment, output limits, timeout, stdin, or resources violate the core domain contract; no child existed. The payload is the typed core error, so the rule that refused the request stays identifiable |
| `InvalidRoot(String)` | root failed to canonicalize or is not a directory; no child existed |
| `InvalidWorkingDirectory(String)` | cwd failed to canonicalize, escapes root, or is not a directory; no child existed |
| `CancelledBeforeSpawn` | cancellation observed before the child ran; no child existed |
| `Setup(String)` | required sandbox/resource setup could not be established, or the launch spec was rejected |
| `RequiredSetupUnavailable` | a required sandbox/resource request has no configured backend |
| `UnsupportedPlatform` | non-Unix host: process-tree control is not implemented; no child existed |
| `Spawn(String)` | the child could not be created or, in the sandbox path, the target could not start |
| `Io(String)` | pipe read or reader-task failure; a child *did* exist |
| `Wait(String)` | process wait failure surfaced as an error rather than a diagnostic |

**How a caller distinguishes completed from refused from failed.** The `Ok`/`Err` split plus the variants give three tiers:

- *Refused before any side effect* — `InvalidRequest`, `InvalidRoot`, `InvalidWorkingDirectory`, `CancelledBeforeSpawn`, `UnsupportedPlatform`. The marker is "no child existed": these all return from the first two gates of `run`.
- *Refused at the enforcement boundary* — `Setup`, `RequiredSetupUnavailable`. A child may have been created and killed in the handshake path, but the *target* never started, and the tests assert this with a marker file (`unsupported_resource_limits_are_reported_or_rejected_before_spawn`).
- *Ran and classified* — `Ok(RunnerResult)`. The `TerminationReason` plus `exit_code` is the answer.
- *Ran and broke* — `Io` / `Wait` mean a child existed but the result could not be assembled. Note that on this path the captured stdout/stderr are lost: `join_reader(...).await?` returns before the `RunnerResult` is constructed.

The server is the reference consumer. `failure_for_runner_error` in the server maps `Spawn` → `ExecutionFailure::Spawn`, `CancelledBeforeSpawn` → `Interrupted`, and everything else — including `UnsupportedPlatform` — → `Internal`, with lease expiry taking precedence over both.

## Concurrency model

`LocalProcessRunner` is a thin newtype over `Box<dyn ExecutionSetup>` (`Send + Sync`), holding no mutable state, so one instance can serve any number of concurrent `run` calls. Callers drive concurrency by spawning: the server wraps `runner.run(...)` in a `tokio::spawn`ed task and joins it in its own `select!` (in the server's execute path).

**Inside one `run`, at most three additional tasks exist:**

| Task | Spawned by | Work |
| --- | --- | --- |
| stdin writer | `tokio::spawn` | `write_all` on the piped stdin handle, then `drop` to close; only for `StdinPolicy::Bytes` |
| stdout reader | `spawn_reader` → `tokio::spawn` | async pipe reads, `BoundedCapture` maintenance, `try_send` streaming, overflow signalling |
| stderr reader | `spawn_reader` → `tokio::spawn` | same, with `stderr: true` |

Plus the calling future, which drives the `select!` and then all the sequential cleanup. stdout and stderr drain genuinely concurrently, which is the only way to avoid the classic deadlock where a child fills stderr while the parent drains only stdout. The `watch` overflow channel is shared by both readers so either stream can trigger termination.

**`spawn_blocking` is not used anywhere in this crate.** There is no database and no unbounded blocking filesystem work here to offload: the only filesystem writes are the bounded (≤ 1 MiB) `spec.json` and `chmod`, and the `systemd-run`/`systemctl` probes go through `tokio::process::Command`, not through blocking calls. The `spawn_blocking` clause in [execution-ownership.md](execution-ownership.md) applies to `eggwork-server` (`store.rs`, `blob.rs`, `workspace.rs`, `artifact.rs`), where SQLite and blob I/O are real blocking work. Using `spawn_blocking` for database or filesystem work is not child-process ownership; spawning a child from it would be.

**Permits.** The runner holds no permit. Per [execution-ownership.md](execution-ownership.md) and ADR-0005 §8, the server acquires its local-admission permit before invoking `run` and releases it exactly once after runner cleanup converges — which is why `run` performs its own process-group cleanup synchronously before returning, and why returning early with `Err` (spawn failure) is a path the server must still release on.

**Cancellation safety.** The token is observed at exactly four points: the pre-spawn check in `LocalProcessRunner::run`, `listener.accept()` and each of the two status reads inside `SandboxChannel::wait`, and the main `select!`. After the `select!` the token is *never* consulted again: SIGTERM, grace, SIGKILL, and reap all run unconditionally. That is deliberate — a cancel arriving mid-cleanup must not be able to abandon a still-live process group. The consequence is that a request which is both cancelled and about to finish is classified by the `biased` precedence (cancellation wins over a concurrent natural exit) and gets exactly one terminal outcome, never two.

Drop-safety is partial by design. Every `Command` sets `.kill_on_drop(true)`, so aborting the `run` task kills the direct child. The process *group* is not reaped on drop, so an abort during the cleanup window can leave descendants for the OS to deal with. The in-source design assumption is that callers join the `run` task rather than aborting it.

## Invariants and enforcement

| Invariant | Enforcement site | Test coverage |
| --- | --- | --- |
| Only approved crates create OS children | `scripts/check_execution_ownership.py` allowlist; `process_group(0)` + `spawn` in `direct_command`/`SandboxChannel::command` | guard script's own negative self-test; `cancellation_before_spawn_and_spawn_failure_are_typed` (typed `Spawn`) |
| A request cannot bypass protocol bounds | private fields; the core rules consumed by `RunnerRequest::contract_violation`; `RunnerRequest::validate` called at the top of `run` | `crates/eggwork-runner/tests/request_validation_parity.rs` — every public construction path reaches the same verdict as `from_spec`, and a refused request never reaches a child |
| cwd is confined to the authorized root | `validate`: canonicalize + `starts_with(&root)` | `cwd_is_confined_and_output_limit_terminates` (positive `pwd`); negative symlink case lives in `crates/eggwork-sandbox-helper/tests/landlock_runner.rs` |
| Child environment is cleared and rebuilt | `env_clear()` in `direct_command`; `build_child_environment`; `baseline_child_environment`; the shared `environment_policy` module | `captures_exit_stdin_and_sanitizes_environment` asserts `LD_PRELOAD` arrives unset; `crates/eggwork-runner/tests/environment_policy.rs` asserts the node-owned keys and the deny-list |
| Stdin is bounded and then closed | `StdinPolicy::validate` + `MAX_STDIN_BYTES`; `write_all` then `drop` | `captures_exit_stdin_and_sanitizes_environment` (bytes path) |
| Capture is bounded with accurate totals | `BoundedCapture::push` | `output_capture_retains_head_tail_with_totals`; `bounds_capture_and_streaming_under_slow_receiver` |
| Pipes stay drained under a slow consumer | `try_send` + drop counter in `spawn_reader` | `bounds_capture_and_streaming_under_slow_receiver` (asserts `stream_chunks_dropped > 0` with a 1-slot channel) |
| Overflow terminates only when configured | `spawn_reader` watch-send gated on `CoreOverflowPolicy::Terminate` | `cwd_is_confined_and_output_limit_terminates` |
| Timeout and cancellation reach descendants | `process_group(0)`, `terminate_group`, `kill_group` | `timeout_and_cancellation_kill_process_group`; `timeout_kills_descendants_that_ignore_sigterm` (`trap '' TERM`); `sandbox_timeout_and_cancellation_reap_the_helper_process_group` (helper crate) |
| Natural exit reaps descendants | post-status `terminate_group` in the `Some(status)` branch | `natural_exit_reaps_background_descendants` |
| A settled output monitor is not a termination | `monitor_termination` returning `Option`; `None` falls through to `child.wait()` | `a_settled_output_monitor_is_not_a_termination_reason` (deterministic); `completed_output_readers_do_not_report_monitor_closed_as_cleanup_failure`; `a_short_lived_child_that_closes_its_pipes_keeps_its_exit_status` (32 iterations) |
| Cleanup never rewrites a known outcome | `execution_result` computes state/failure before touching `cleanup` | `cleanup_warning_does_not_rewrite_process_outcome` |
| Secrets never reach `Debug` | hand-written `Debug` for `RunnerRequest`, `StdinPolicy`, `OutputChunk`, `BoundedCapture` | `runner_request_debug_redacts_command_and_inputs` (argv, env value, stdin) |
| Required setup fails closed before the target runs | `TrustedLandlockSetup::prepare`; `NoExecutionSetup::prepare`; `SandboxChannel::wait` decode; `wrap_resource_command` backend match | `unsupported_resource_limits_are_reported_or_rejected_before_spawn` (marker file absent); `helper_trust_checks_reject_missing_wrong_and_symlinked_helpers` and `malformed_private_spec_and_unavailable_status_channel_do_not_launch_target` (helper crate) |
| Advisory capability == enforcement answer | one `verify_trusted_helper`; `probe_landlock_ruleset` mirrors the helper's rules; delegation in `operations::trusted_helper` | `trusted_helper_advertises_landlock_workspace_rw_capability`; `missing_helper_advertises_no_landlock_capability`; `no_setup_advertises_empty_dynamic_capabilities`; `capability_feature_string_is_frozen_for_workspace_rw_profile` |
| Pre-spawn refusals are typed and distinguishable | the two early returns in `run`; `failure_for_runner_error` in the server | `cancellation_before_spawn_and_spawn_failure_are_typed`; `unsupported_resource_limits_are_reported_or_rejected_before_spawn` |
| A degradation is reported in the stream, before the target's output | `report_setup_degradations` after `prepare`, in the `SandboxChannel::new` failure arm, and in the handshake recovery arms; `OutputChunk::Diagnostic` on the same channel as the target's output | `crates/eggwork-runner/tests/diagnostic_degradation.rs` (the diagnostic is the first chunk on the channel); server-side `a_degraded_best_effort_execution_publishes_its_diagnostic_before_output_and_terminal` (sequence numbers) |
| A required request is never reported as a degradation | `report_setup_degradations` only reports `BestEffort` requests; `prepare` errors on a required request before any diagnostic could be emitted | `a_required_sandbox_is_refused_without_a_degradation_diagnostic`; server-side `a_required_sandbox_never_reaches_the_target_and_is_not_reported_as_a_degradation` |
| Cross-platform honesty | `#[cfg(not(unix))]` refusal; non-Linux fail-closed fallbacks | none in this crate (see Test coverage) |

## Platform support matrix

| Platform | What works | What fails closed | Cited source |
| --- | --- | --- | --- |
| Linux (x86_64, the exercised path) | Full lifecycle: `process_group(0)`, `killpg` termination and reaping, direct or helper-wrapped launch, Landlock `workspace_rw` via the trusted helper, systemd cgroup-v2 resource wrapping, helper status-byte and `Result=oom-kill` limit attribution, `systemctl stop` unit cleanup. Default setup is `TrustedLandlockSetup::discover_sibling()`. | Untrusted/missing helper → no `isolation.landlock.workspace-rw.v1`; required sandbox → `Err(Setup)`; required resources with an unavailable backend → `Err(Setup)`; `#[cfg]`-gated deps keep everything else out of the build | `TrustedLandlockSetup`, `verify_trusted_helper`, `SandboxChannel`, `SystemdCgroupBackend` |
| macOS / other Unix | Process lifecycle only: `direct_command`, `process_group(0)`, `killpg` SIGTERM/SIGKILL, bounded capture, timeout/cancellation/overflow, `UnsupportedPlatform` not applicable. Default setup is `NoExecutionSetup`. | `verify_trusted_helper` → `Err("trusted Landlock is unsupported on this platform")`; `probe_landlock_ruleset` → `false`; `SystemdCgroupBackend::probe` → `Err("OS resource controls are unsupported on this platform")`; required sandbox or resources → `Err(RequiredSetupUnavailable)`; capability advertisement is empty | `SystemdCgroupBackend`, `probe_landlock_ruleset`, `verify_trusted_helper`, `NoExecutionSetup` |
| Windows | Full lifecycle through the Job Object backend: pre-run job assignment (`CREATE_SUSPENDED` → assign → resume), `TerminateJobObject` convergence for leader exit, timeout, cancellation, and output limit, bounded capture, stdin delivery, the `#[cfg(windows)]` environment baseline. Default setup is `NoExecutionSetup`. | Required sandbox or resources → `Err(RequiredSetupUnavailable)` (Windows has no Landlock and no cgroup backend), capability advertisement is empty, and the receipt reports `sandbox: NotRequested` rather than claiming confinement. | `process_tree.rs` (`cfg(windows)`), `windows_process_tree.rs` |
| Any other non-Unix compile target | Request validation only — `RunnerRequest::validate` still runs, so bounds and root/cwd confinement are enforced. | `Err(RunnerError::UnsupportedPlatform)` ("process-tree control is unsupported on this platform") raised by `ProcessTree::spawn_platform`; the server maps it to `ExecutionFailure::Internal`. | `process_tree.rs` (`cfg(not(any(unix, windows)))`) |
| Any platform, `NoExecutionSetup` | Direct launch, full lifecycle | `Err(RequiredSetupUnavailable)` for required sandbox or required resources; empty dynamic capability list | `NoExecutionSetup` |

Per the closure evidence in `plans/closure/security-isolation-resource/004-status.md`, only the Linux host class is isolation-qualified. Foundation M004 (see `plans/closure/foundation-execution-core/004-status.md`) qualified the Windows *execution* path on a hosted `windows-latest` runner; Windows remains isolation-free by construction, because the only isolation backend in this repository is Landlock, and that backend fails closed there rather than degrading. `landlock` is a Linux-only kernel API and the `Cargo.toml` comment says so: "every required release target that is not Linux builds this crate, and all `landlock::` uses are already `#[cfg(target_os = "linux")]`-gated with fail-closed fallbacks."

## Dependencies

| Dependency | Scope | Why it is here |
| --- | --- | --- |
| `eggwork-core` | always | The domain contract: `ExecutionSpec`/`CommandSpec`, `ExecutionResult`, `ExecutionState`, `ExecutionFailure`, `ExecutionId`/`ExecutionGeneration`, `RelativePath`, `Requirement`, `StdinPolicy`, `OverflowPolicy`, `ResourceDimension`, and every limit constant (`MAX_ARG_COUNT`, `MAX_ARG_BYTES`, `MAX_ENV_COUNT`, `MAX_ENV_NAME_BYTES`, `MAX_ENV_VALUE_BYTES`, `MAX_STDIN_BYTES`, `MAX_CAPTURE_BYTES`, `MAX_EVENT_CHUNK_BYTES`, `MAX_TIMEOUT`). The runner owns no limits of its own. |
| `tokio` (`features = ["net", "process"]`) | always | `net` for `UnixListener`/`UnixStream` in the sandbox channel; `process` for `tokio::process::Command`/`Child`. Both are the crate's only OS-facing surface. |
| `tokio-util` | always | `CancellationToken` — the only lifetime signal the runner accepts. |
| `async-trait` | always | `ExecutionSetup` is an async trait used as a `Box<dyn>` object behind `LocalProcessRunner`. |
| `thiserror` | always | `RunnerError`'s `#[error(...)]` messages. |
| `serde` + `serde_json` | always | `SandboxLaunchSpec` is `Serialize` in the runner and `Deserialize` (`deny_unknown_fields`) in the helper. This is the only serialization in the crate, and it is the runner-to-helper wire format. |
| `tempfile` | always (also a dev-dependency) | Production: `SandboxChannel`'s private `0700` directory holding `spec.json` and `status.sock`. Dev: test roots and the copied helper in `capability_probe.rs`. |
| `landlock` (`=0.4.7`) | `[target.'cfg(target_os = "linux")'.dependencies]` | Kernel Landlock ABI V4 ruleset construction for the capability probe. |
| `nix` (`features = ["user"]` plus workspace `signal`, `process`) | `[target.'cfg(unix)'.dependencies]` | `killpg` for the Unix tree convergence, `geteuid` for the trust check and the systemd user-manager decision. |
| `process-wrap` (`=10.0.1`, `default-features = false`, features `tokio1`, `job-object`, `kill-on-drop`) | `[target.'cfg(windows)'.dependencies]` | Windows Job Object process-tree ownership. It is on the guard's `FORBIDDEN_PROCESS_DEPS` denylist and allowed here only through the recorded `APPROVED_PROCESS_DEP_EXCEPTIONS` entry in `scripts/check_execution_ownership.py`, because it is the reviewed answer to a Windows-only problem Eggwork cannot solve with a bare `tokio::process::Child`: the Job Object has to exist before the target's first instruction. The pin is exact on purpose — upstream raises its MSRV inside a major version, so `10` is not a stable lower bound. `creation-flags` and `process-group` are deliberately *not* enabled: Eggwork passes no user creation flags, so the wrapper's default resume-after-assignment behaviour is exactly right. |

**Why the two are target-scoped.** Landlock is a Linux kernel LSM interface; there is no equivalent on any other kernel, so a workspace-wide dependency would either fail to build elsewhere or drag an unused crate into every release target — and `Eggpack` ships a `windows-x64` target, so this crate is compiled on non-Linux. `nix` covers all Unix, not just Linux, because the two things it provides here (`killpg` and `geteuid`) exist on macOS too and are exactly what the non-Linux *unix* path needs. Both are therefore gated at the same granularity as the `#[cfg]` blocks that use them: `landlock` at `target_os = "linux"`, `nix` at `unix`. `process-wrap` is gated at `windows` for the same reason. Non-Linux-unix pulls none of them, and gets the fail-closed stubs instead. `scripts/check_execution_ownership.py` forbids the process-spawn helper crates (`command-group`, `duct`, `process-wrap`, `portable-pty`, `subprocess`) in every crate except through a recorded `APPROVED_PROCESS_DEP_EXCEPTIONS` entry, and asserts that `eggwork-core` and `eggwork-client` do not depend on `eggwork-runner`. Before M004 the denylist was only applied to `eggwork-server`, which left the other crates unchecked; the exception table is what makes applying it everywhere safe.

## Test coverage

`mod tests` contains 29 test functions plus a request helper; the table below is a selection of them, not the whole suite. It constructs `RunnerRequest` by *literal struct expression*, which only compiles because the tests are inside the module that owns the private fields — itself a small proof that privacy is enforced by the compiler rather than by lint.

| Test | Asserts |
| --- | --- |
| `a_settled_output_monitor_is_not_a_termination_reason` | `monitor_termination(true) == Some(OutputLimit)`, `monitor_termination(false) == None` — the deterministic guard against the `exit_code: null` regression |
| `a_short_lived_child_that_closes_its_pipes_keeps_its_exit_status` | 32 iterations of a short-lived child writing to both pipes always yield `Exited` + `Some(0)` + exact stdout + `wait_error == None`; has `cfg!(windows)` argv branches even though Windows refuses before spawn |
| `unsupported_resource_limits_are_reported_or_rejected_before_spawn` | best-effort memory with `NoExecutionSetup` runs `/bin/true` and reports `ResourceDimensionResult::NotApplied`; required pids returns `RequiredSetupUnavailable` **and** the target marker file does not exist |
| `runner_request_debug_redacts_command_and_inputs` | `argv-secret`, `env-secret`, `stdin-secret` are all absent from `format!("{request:?}")` |
| `captures_exit_stdin_and_sanitizes_environment` | stdin bytes are delivered, `LD_PRELOAD` is stripped (child prints `input:unset`), stderr captured separately, exit code 7 preserved |
| `completed_output_readers_do_not_report_monitor_closed_as_cleanup_failure` | 16 short-lived runs keep exit status and clean `wait_error`; a child that closes its pipes *before* doing work still completes and writes its marker |
| `bounds_capture_and_streaming_under_slow_receiver` | 200 KB of output with `capture_limit = 128`, `event_chunk_bytes = 64`, and a **1-slot** channel: `total_bytes >= 200000`, retained bytes exactly 128, `stream_chunks_dropped > 0` |
| `characterize_output_drain_overhead` | `#[ignore]`d manual characterization; prints throughput for 4 KiB and 8 MiB, asserts only that `total_bytes` is exact and retention is bounded |
| `timeout_and_cancellation_kill_process_group` | 100 ms timeout on `sleep 30 & wait` → `TimedOut`; cancel from another task → `Cancelled` |
| `cancellation_before_spawn_and_spawn_failure_are_typed` | pre-cancelled token → `CancelledBeforeSpawn`; missing binary → `Spawn(_)` |
| `cwd_is_confined_and_output_limit_terminates` | `cwd` resolves to the expected directory under root; an infinite-output child with `Terminate` and `capture_limit = 64` → `OutputLimit` |
| `timeout_kills_descendants_that_ignore_sigterm` | Linux-only: `trap '' TERM; sleep 30 & echo $!; wait`, 100 ms timeout → the descendant is gone or `Z` in `/proc/<pid>/stat` |
| `natural_exit_reaps_background_descendants` | Linux-only: a leader that exits leaving `sleep 30` behind still reports `Exited` and the background pid is gone or `Z` |
| `output_capture_retains_head_tail_with_totals` | `push(b"abcdefghij", 6)` → head `abc`, tail `hij`, omitted `4` |
| `cleanup_warning_does_not_rewrite_process_outcome` | `Exited`/`Some(9)` with a signal error → `Failed`, `exit_code: Some(9)`, `cleanup_warning: Some("permission denied")` |
| `capability_feature_string_is_frozen_for_workspace_rw_profile` | the constant is literally `"isolation.landlock.workspace-rw.v1"` |
| `no_setup_advertises_empty_dynamic_capabilities` | `NoExecutionSetup::execution_capabilities()` is empty |

`crates/eggwork-runner/tests/windows_process_tree.rs` (`#![cfg(windows)]`, run by the `windows-runner` CI job) is the Windows-only tree suite, and it is deliberately not a mirror of the Unix tests: the Unix ones use `/bin/sh`, which does not exist on Windows. Every tree claim is proven by a fixture that appends a heartbeat file on a 50 ms cadence, sampled *after* the runner returns, because there is no safe-Rust process-liveness API here. `assert_frozen` first asserts the heartbeat file is non-empty so the proof cannot pass vacuously, then re-samples after 600 ms. The proofs are: direct exit with CRLF-captured stdout/stderr and exit code 3; a descendant created at startup owned after the leader exits; timeout, cancellation, and output-limit convergence for both a leader and a detached descendant; leader exit not releasing a live descendant; leader exit status not rewritten by descendant termination; working directory honoured *and* a write outside the root succeeding with `sandbox: NotRequested`; stdin delivered and closed; typed spawn failure with no marker left behind; descendant output captured (pipe inheritance); the documented environment baseline with no ambient leak; bare program name resolved through `PATH`; required isolation and required resources refused before the marker command runs, with best-effort reported `NotApplied`; and five repeated timeout convergences, because tree bugs are timing bugs.

`tests/capability_probe.rs` covers the advertisement boundary end to end. It copies the built `eggwork-sandbox-helper` out of `$CARGO_TARGET_DIR/debug` (or `OUT_DIR`) into a fresh `0700` tempdir as `0755`, deliberately leaks the tempdir, and asserts `LocalProcessRunner::new(TrustedLandlockSetup::new(helper)).execution_capabilities()` contains `isolation.landlock.workspace-rw.v1` on an ABI-V4 kernel. It **skips** (prints and returns) when no helper binary is present. The mirror case, `missing_helper_advertises_no_landlock_capability`, points at `/nonexistent/eggwork-sandbox-helper` and asserts the feature is absent.

`crates/eggwork-runner/tests/diagnostic_degradation.rs` is the degradation-reporting suite. It drives the runner through its public API with `LocalProcessRunner::new(NoExecutionSetup)` — chosen explicitly, because `LocalProcessRunner::default()` picks a setup from the environment and a test that wanted "no enforcement available" could silently get an enforcing one. The argv is platform specific (`cmd.exe` on Windows, `/bin/sh` elsewhere) because the `windows-runner` CI job runs this suite and the runner has platform-specific spawn paths the diagnostic sits in front of. The proofs are: a degraded best-effort sandbox emits exactly one diagnostic and it is the *first* chunk on the channel, ahead of the target's output; a degraded best-effort resource request emits one diagnostic naming the affected dimension, and two affected dimensions emit two diagnostics; a required sandbox is refused with `RequiredSetupUnavailable`, the target's marker file does not exist, and no diagnostic is emitted; an execution with nothing degraded emits no diagnostic; a cancellation before the spawn emits no diagnostic; and every emitted payload stays within `MAX_DIAGNOSTIC_PAYLOAD_BYTES` and parses against the documented grammar.

`SandboxChannel` internals are covered from the helper crate's integration test, `crates/eggwork-sandbox-helper/tests/landlock_runner.rs`, which drives the runner's public API: outside read/write denial, helper trust rejections (missing/wrong/symlinked), workspace and cwd symlink escapes, timeout and cancellation reaping the helper's own process group, required memory/PID/CPU enforcement and classification, concurrent cgroup scope isolation, and `malformed_private_spec_and_unavailable_status_channel_do_not_launch_target`.

`crates/eggwork-runner/tests/request_validation_parity.rs` is the request-contract suite. It drives the runner through its **public** API only — the builder path (`RunnerRequest::new` plus the setters) and the protocol path (`from_spec`) — because a test that only exercised `eggwork_core` would have passed before the corrective and would have proved nothing about the runner. The proofs are: every public construction path reaches the same verdict as `from_spec` for the same input, with the same typed error; every environment-name form core rejects is refused through the runner, with the refusal attributable to the name rule; a repeated environment name is refused by the duplicate-name rule specifically; the runner refuses an empty argv entry that core permits, which is the one place it is stricter; the stdin and output bounds are core's bounds; the mutable resource door is covered even though it cannot validate on the way in; a refused request never reaches a child (proved with a marker file); and the public surface of `RunnerRequest` is pinned by a test that fails if a new public entry point is added. `crates/eggwork-core/tests/environment_rule_partition.rs` is the other half: it re-derives the documented environment-entry rule independently of the implementation and proves that the accepted and rejected sets partition the candidate space, so a rule change cannot silently open a gap.

`crates/eggwork-runner/tests/environment_policy.rs` is the environment-policy suite on the runner's side: it asserts that the shared policy is reachable from the runner's public API, that the node-owned class and the node-owned baseline are the same list, that the baseline the runner installs is the policy's baseline, and — through a real child — that a caller's value for a node-owned key does not reach the child while a denied key does not either and a caller-controlled key does. The other half of that proof is `crates/eggwork-sandbox-helper/tests/environment_policy_parity.rs`, which drives the real helper binary: it compares the two generated copies of the policy element by element, drives the helper with a launch specification for every probe key and compares its accept/refuse decision with what the runner's copy of the policy says, proves that a caller-supplied node-owned key keeps the execution confined, and proves that the direct path and the helper path build the same environment.

**Thin areas, honestly:**

- **The `RunnerError::InvalidRequest` branches are now exercised; the root/cwd rules still are not.** `crates/eggwork-runner/tests/request_validation_parity.rs` covers every core-owned rule the runner consumes — empty/NUL/oversized argv, the argv count bound, the environment count/name/value/duplicate rules, `capture_limit` over 16 MiB, `event_chunk_bytes` of 0 or over 64 KiB, a zero or over-maximum timeout, stdin over 4 MiB, and an out-of-range resource requirement — through the runner's public API, and asserts that a refused request never reaches a child. The root/cwd resolution is the remaining gap, and it is the bullet below.
- **No test covers a cwd that escapes root** (`..`, absolute, or a symlink) at the runner level; that assertion lives in the helper crate's test, and only for the symlink form.
- **No test covers `RunnerError::UnsupportedPlatform`.** The path is now `cfg(not(any(unix, windows)))`, which is not a release target, so the refusal stays asserted only by code reading. Windows no longer takes this path.
- **Pre-run Job Object assignment is a property of the dependency, not of a test.** `process-wrap` adds `CREATE_SUSPENDED`, assigns, and resumes inside `spawn`; the suite can observe the consequences (a descendant created in the target's first moments is owned, a spawn failure leaves nothing running) but cannot observe the assignment itself from outside the process without Win32 tracing. That is stated rather than glossed: the evidence is "no descendant ever escaped", not "the kernel was entered in this state".
- **The baseline environment is only tested negatively.** `captures_exit_stdin_and_sanitizes_environment` proves `LD_PRELOAD` is stripped. The exact baseline set, the deny-list, and the `PATH` special case are now pinned by `crates/eggwork-runner/tests/environment_policy.rs` and by the sandbox helper's parity suite, but only on Unix: the Windows baseline has no coverage outside the `windows-runner` job.
- **`SystemdCgroupBackend::wrap` argv construction is untested** — the `--property` rendering, the `MemoryMax`/`MemorySwapMax=0` pairing, the `CPUQuota` millis-to-percentage conversion, `resource_unit_name` uniqueness, and the `--user` selection are all unasserted. The resource *enforcement* tests live in the helper crate and depend on a systemd-capable host.
- **`BoundedCapture` is tested at exactly one limit (6).** Odd limits, `limit == 1`, and `limit == 0` are unverified, as is the head/tail split when a single `push` straddles the boundary.
- **`stream_chunks_dropped` semantics are untested** beyond `> 0`; the closed-channel case (dropping the receiver mid-run) is exercised only by `drop(rx)` after the assertion.
- **No cancel-versus-natural-exit race test**, even though plan 002 §6 lists one as required. The `biased` precedence makes the outcome deterministic, but nothing asserts which side wins.
- **`SandboxWaitError::AfterTarget` and the `Ok(None)` respawn path have no coverage in this crate**; the marker-file assertion in `unsupported_resource_limits_...` covers the required-refusal case only.

## Review focus

- **Descendant reaping completeness.** `killpg` only reaches processes that stayed in the child's process group. A grandchild that calls `setsid()` or `setpgid()` escapes both `terminate_group` and `kill_group`, and nothing here detects that. Plan 002 §9 says to stop rather than "pretend direct-child kill is tree cleanup" — that boundary should be re-checked against the current code, which is group-scoped, not tree-scoped.
- **The best-effort respawn path re-runs argv outside the wrapper.** On `Ok(None)` or a recoverable `BeforeTarget` error, the helper child is `start_kill`ed and waited, but *its process group is not signalled* and the command is then rebuilt and spawned a second time, unwrapped. Whether a best-effort request should ever be able to produce two process creations for one execution is a policy question the code currently answers implicitly.
- **Env baseline duplication — closed, but the shape of the fix matters.** The baseline variable set used to appear twice — `baseline_child_environment()` and the hard-coded `environment` vector in `SandboxChannel::new` — and the deny-list appeared here and again in the helper, so a sandbox child and a non-sandbox child could drift apart silently. Both now come from one source of truth, `crates/eggwork-runner/policy/environment-policy.toml`, generated into each crate, and one builder (`build_child_environment`) produces both launch paths' environments. What the fix does *not* do is make the deny-list exhaustive: adding a knob is still a policy decision with its own security review, and the generated copies are only as good as the source they are generated from.
- **Head/tail capture correctness under concurrent drain.** `BoundedCapture::push` is byte-at-a-time and interleaves head-fill with tail-fill in one pass. Check the `div_ceil` head bias, `limit == 0`/`limit == 1` degeneracy, and that a large single read cannot produce a tail that overlaps the head.
- **`stream_chunks_dropped` counts bytes, not chunks.** `dropped += bytes.len() as u64` accumulates a byte count into a field named `stream_chunks_dropped`. The value is correct as a "bytes not delivered" measure and misleading as a chunk count; either the arithmetic or the name should change.
- **Trust-check TOCTOU.** `verify_trusted_helper` is a path-based `symlink_metadata` plus an ancestor walk, and the helper is executed later by path. Nothing pins an fd between check and exec, and `SandboxChannel::command()` re-resolves the path. The window is small but real, and it is the window the whole isolation story depends on.
- **Grace-period sizing and where it is spent.** The same 300 ms is both the `SIGTERM → SIGKILL` delay and the post-`SIGKILL` wait bound, so the worst case adds ~600 ms; a `systemctl stop` adds up to 2 s more. More importantly, a *successful* run that leaves any descendant alive pays a full 300 ms. Whether these are the right constants for the workload is a tuning question the code has not revisited since M002.
- **`cleanup_warning` priority can mask a more relevant diagnostic.** `process_group_signal_error` is pre-seeded with the `systemctl stop` failure, and it outranks `wait_error` in the single folded `cleanup_warning` string. The individual fields survive in `RunnerResult::cleanup`, so nothing is lost — but the field most operators will read can be about cgroup cleanup rather than about a failed `wait()`.
- **Windows converges the tree without observing it.** `TerminateJobObject` is sound for the invariant that matters — no member can execute user code after it returns — but Eggwork cannot report *which* descendants died or observe a job-membership count, because no safe surface is available. A reader who wants per-descendant evidence will not find it here, and should not assume the Job Object path is as observable as the process-group path.
- **Non-Linux honesty.** The non-Linux `verify_trusted_helper` reports "trusted Landlock is unsupported" rather than "this platform runs children without isolation". Both are true on Windows now that `run` proceeds, but a reader skimming the fallbacks could still come away with a wrong model of what Windows does — Windows now executes children and refuses isolation, which is a different thing from refusing to execute at all.
- **Can cleanup mask a real exit status?** It cannot change `termination` or `exit_code` — the `Some(status)` branch returns the known status regardless. But `Err(Io)` from `join_reader` discards the `BoundedCapture` entirely, so a late pipe error loses the child's output and totals on a run that otherwise succeeded. Whether a partially captured result is better than an error is a deliberate-or-accidental design choice worth confirming.
- **The `biased` select makes cancellation outrank natural exit.** An execution that completes in the same instant a cancel arrives is reported `Cancelled`, and its exit code is discarded. This yields exactly one terminal outcome (ADR-0003 is satisfied), but the winner is precedence, not truth, and no test pins it.

## Related

- [Architecture overview](overview.md) — component map and the normative invariant list.
- [Sandbox helper deep dive](sandbox-helper.md) — the other half of the isolation story: `LaunchSpec` validation, Landlock rule application, and `execve`.
- [Execution ownership](execution-ownership.md) — the normative rule, the helper exception, and the `spawn_blocking` clarification.
- [ADR-0005: Runner/Service Separation and Protective Local Admission](../plans/adrs/ADR-0005-runner-service-separation-and-local-admission.md) — why the runner exists as a separate boundary, and the permit lifetime rule.
- [ADR-0003: Execution Idempotency, Ownership Leases, and Generation Fencing](../plans/adrs/ADR-0003-execution-idempotency-leases-and-fencing.md) — cancellation, terminal single-assignment, and the "cleanup diagnostics may be appended without changing the chosen terminal outcome" rule.
- [Plan 002: Canonical Local Runner](../plans/implementation/foundation-execution-core/002-canonical-local-runner.md) — the implementation contract, required tests, and stop conditions this crate was built against.
- [Plan 003: Execution Ownership Guards and Runner API Hardening](../plans/implementation/foundation-execution-core/003-execution-ownership-guards-and-runner-api-hardening.md) — the source guard and the private-field hardening.
