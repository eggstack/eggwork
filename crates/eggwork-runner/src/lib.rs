#![forbid(unsafe_code)]

//! Canonical owner of finite, noninteractive process execution.
//!
//! Build validated [`RunnerRequest`] values from [`eggwork_core::ExecutionSpec`]
//! and run them through [`LocalProcessRunner`]. Requests remain private and are
//! rejected before setup or spawn when invalid. [`ExecutionSetup`] supplies
//! platform policy; scheduling remains caller-owned.

use eggwork_core::{
    CommandSpec, EnvironmentEntry, ExecutionFailure, ExecutionGeneration, ExecutionId,
    ExecutionResult, ExecutionSpec, ExecutionState, MAX_CAPTURE_BYTES, MAX_EVENT_CHUNK_BYTES,
    OutputPolicy, OverflowPolicy as CoreOverflowPolicy, RelativePath, Requirement,
    ResourceRequirements, ValidationError, validate_argv, validate_environment_entries,
    validate_timeout,
};
// The runner consumes the core domain type rather than keeping a local copy of
// it. The public path `eggwork_runner::StdinPolicy` is retained as a re-export
// so that embedders and the in-tree suites keep compiling against the path they
// already use; the type that path names is `eggwork_core::StdinPolicy`, and the
// runner's former local duplicate -- with its own `Debug` and its own copy of
// the stdin bound -- is gone.
pub use eggwork_core::StdinPolicy;
#[cfg(target_os = "linux")]
use serde::Serialize;
#[cfg(target_os = "linux")]
use std::io::Write;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use thiserror::Error;
#[cfg(target_os = "linux")]
use tokio::net::UnixListener;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, watch},
    task::JoinHandle,
    time::sleep,
};
use tokio_util::sync::CancellationToken;

mod process_tree;

/// Generated environment policy owned by `policy/environment-policy.toml`.
///
/// The standalone helper has its own generated copy; a parity test prevents
/// the two enforcement paths from drifting.
pub mod environment_policy;

use process_tree::{ProcessTree, TreeConvergence};

const READ_BUFFER: usize = 8192;
const TERMINATION_GRACE: Duration = Duration::from_millis(300);

/// Neutral attribution carried into the child environment and result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecutionProvenance {
    pub execution_id: Option<ExecutionId>,
    pub generation: Option<ExecutionGeneration>,
}

/// Arbitrary output bytes or an enforcement diagnostic.
///
/// Diagnostics share the ordered output channel, so one emitted before spawn
/// reaches the caller before target output.
#[derive(Clone, PartialEq, Eq)]
pub enum OutputChunk {
    /// Target standard output.
    Stdout(Vec<u8>),
    /// Target standard error.
    Stderr(Vec<u8>),
    /// A bounded diagnostic payload about the execution's isolation or
    /// resource enforcement. See [`eggwork_core::DiagnosticPayload`] for the
    /// grammar and the bound.
    Diagnostic(String),
}
impl std::fmt::Debug for OutputChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdout(bytes) => f
                .debug_struct("OutputChunk")
                .field("stream", &"stdout")
                .field("length", &bytes.len())
                .field("bytes", &"[REDACTED]")
                .finish(),
            Self::Stderr(bytes) => f
                .debug_struct("OutputChunk")
                .field("stream", &"stderr")
                .field("length", &bytes.len())
                .field("bytes", &"[REDACTED]")
                .finish(),
            Self::Diagnostic(payload) => f
                .debug_struct("OutputChunk")
                .field("stream", &"diagnostic")
                .field("length", &payload.len())
                .field("payload", &"[REDACTED]")
                .finish(),
        }
    }
}

/// Head and tail retention with accurate byte accounting.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct BoundedCapture {
    pub head: Vec<u8>,
    pub tail: VecDeque<u8>,
    pub total_bytes: u64,
    pub omitted_bytes: u64,
}
impl std::fmt::Debug for BoundedCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundedCapture")
            .field("total_bytes", &self.total_bytes)
            .field("omitted_bytes", &self.omitted_bytes)
            .field("captured_bytes", &(self.head.len() + self.tail.len()))
            .field("content", &"[REDACTED]")
            .finish()
    }
}

impl BoundedCapture {
    fn push(&mut self, bytes: &[u8], limit: usize) {
        let head_limit = limit.div_ceil(2);
        let tail_limit = limit.saturating_sub(head_limit);
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        // Bulk, not per-byte. Once the head is full only the *last* `tail_limit`
        // bytes of the incoming slice can survive: everything between is evicted
        // by the byte after it. Walking `bytes` one element at a time made a 1 GB
        // stdout a billion-iteration `VecDeque` pop/push loop even though the
        // overwhelming majority of those iterations were immediately discarded.
        let remaining = if self.head.len() < head_limit {
            let room = head_limit - self.head.len();
            let take = room.min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
            &bytes[take..]
        } else {
            bytes
        };
        if tail_limit == 0 {
            // Nothing after the head is retained.
        } else if remaining.len() >= tail_limit {
            // The tail is exactly the final `tail_limit` bytes of everything
            // retained so far, so replace it wholesale.
            let window = &remaining[remaining.len() - tail_limit..];
            self.tail.clear();
            self.tail.extend(window.iter().copied());
        } else if self.tail.len() + remaining.len() <= tail_limit {
            self.tail.extend(remaining.iter().copied());
        } else {
            // Keep only what fits: the old tail's tail plus all of `remaining`.
            let keep_from_old = tail_limit - remaining.len();
            let mut merged = VecDeque::with_capacity(tail_limit);
            merged.extend(
                self.tail
                    .iter()
                    .copied()
                    .skip(self.tail.len().saturating_sub(keep_from_old)),
            );
            merged.extend(remaining.iter().copied());
            self.tail = merged;
        }
        self.omitted_bytes = self
            .total_bytes
            .saturating_sub((self.head.len() + self.tail.len()) as u64);
    }
}

/// The original per-byte `BoundedCapture::push`, kept as a test oracle.
///
/// The production `push` works in bulk and is only allowed to differ from this
/// in cost, never in result. See
/// `bounded_capture_bulk_push_matches_the_per_byte_oracle`.
#[cfg(test)]
fn push_per_byte(
    head: &mut Vec<u8>,
    tail: &mut VecDeque<u8>,
    total_bytes: &mut u64,
    omitted_bytes: &mut u64,
    bytes: &[u8],
    limit: usize,
) {
    let head_limit = limit.div_ceil(2);
    let tail_limit = limit.saturating_sub(head_limit);
    *total_bytes = total_bytes.saturating_add(bytes.len() as u64);
    for &byte in bytes {
        if head.len() < head_limit {
            head.push(byte);
        } else if tail_limit > 0 {
            if tail.len() == tail_limit {
                tail.pop_front();
            }
            tail.push_back(byte);
        }
    }
    *omitted_bytes = total_bytes.saturating_sub((head.len() + tail.len()) as u64);
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn bounded_capture_bulk_push_matches_the_per_byte_oracle() {
        // The bulk implementation only skips work that the per-byte loop would
        // have discarded on the next byte, so the retained head, tail, total and
        // omitted counters must be identical for every chunking — including the
        // cases that straddle the head/tail boundary, the tail filling up
        // mid-slice, and a `limit` of zero or one.
        for limit in [0usize, 1, 2, 3, 7, 16, 64] {
            let mut fast = BoundedCapture {
                head: Vec::new(),
                tail: VecDeque::new(),
                total_bytes: 0,
                omitted_bytes: 0,
            };
            let mut slow = BoundedCapture {
                head: Vec::new(),
                tail: VecDeque::new(),
                total_bytes: 0,
                omitted_bytes: 0,
            };
            let mut sequence: Vec<u8> = Vec::new();
            // Chunk sizes deliberately cross every internal threshold.
            for chunk in [1usize, 3, 5, 8, 13, 21, 64] {
                sequence.clear();
                for index in 0..chunk * 5 {
                    sequence.push((index % 251) as u8);
                }
                fast.push(&sequence, limit);
                push_per_byte(
                    &mut slow.head,
                    &mut slow.tail,
                    &mut slow.total_bytes,
                    &mut slow.omitted_bytes,
                    &sequence,
                    limit,
                );
                assert_eq!(
                    fast.head, slow.head,
                    "head diverged at limit={limit} chunk={chunk}"
                );
                assert_eq!(
                    fast.tail, slow.tail,
                    "tail diverged at limit={limit} chunk={chunk}"
                );
                assert_eq!(fast.total_bytes, slow.total_bytes);
                assert_eq!(fast.omitted_bytes, slow.omitted_bytes);
            }
            // An empty push must be a no-op in both.
            fast.push(&[], limit);
            assert_eq!(fast.total_bytes, slow.total_bytes);
        }
    }

    #[test]
    fn bounded_capture_keeps_the_head_and_the_last_tail_bytes() {
        let mut capture = BoundedCapture {
            head: Vec::new(),
            tail: VecDeque::new(),
            total_bytes: 0,
            omitted_bytes: 0,
        };
        // limit 8 => head 4, tail 4.
        capture.push(b"abcdefghij", 8);
        assert_eq!(capture.head, b"abcd");
        assert_eq!(capture.tail.iter().copied().collect::<Vec<_>>(), b"ghij");
        assert_eq!(capture.total_bytes, 10);
        assert_eq!(capture.omitted_bytes, 2);
    }
}

/// Typed optional setup hook. Best-effort outcomes are surfaced and never
/// described as enforced. The trusted sandbox implementation is a later plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxRequest {
    None,
    BestEffort { profile: String },
    Required { profile: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxOutcome {
    NotRequested,
    NotApplied { reason: String },
    Applied { profile: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSetupRequest {
    pub memory_bytes: Requirement<u64>,
    pub cpu_millis: Requirement<u64>,
    pub pids: Requirement<u32>,
}

impl ResourceSetupRequest {
    fn is_requested(&self) -> bool {
        !matches!(self.memory_bytes, Requirement::NotRequested)
            || !matches!(self.cpu_millis, Requirement::NotRequested)
            || !matches!(self.pids, Requirement::NotRequested)
    }

    fn has_required(&self) -> bool {
        matches!(self.memory_bytes, Requirement::Required(_))
            || matches!(self.cpu_millis, Requirement::Required(_))
            || matches!(self.pids, Requirement::Required(_))
    }

    /// systemd transient-unit properties for the requested limits. Only the Linux
    /// helper handshake builds them, so the method is Linux-shaped.
    #[cfg(target_os = "linux")]
    fn properties(&self) -> Vec<(String, String)> {
        let mut properties = Vec::new();
        if let Requirement::BestEffort(value) | Requirement::Required(value) = self.memory_bytes {
            properties.push(("MemoryMax".into(), value.to_string()));
            properties.push(("MemorySwapMax".into(), "0".into()));
        }
        if let Requirement::BestEffort(value) | Requirement::Required(value) = self.cpu_millis {
            let whole = value / 10;
            let fractional = value % 10;
            properties.push(("CPUQuota".into(), format!("{whole}.{fractional}%")));
        }
        if let Requirement::BestEffort(value) | Requirement::Required(value) = self.pids {
            properties.push(("TasksMax".into(), value.to_string()));
        }
        properties
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct SystemdCgroupBackend {
    systemd_run: PathBuf,
    user_manager: bool,
}

#[cfg(target_os = "linux")]
impl SystemdCgroupBackend {
    fn discover() -> Result<Self, String> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let systemd_run = ["/usr/bin/systemd-run", "/bin/systemd-run"]
            .iter()
            .find_map(|path| Path::new(path).canonicalize().ok())
            .ok_or_else(|| "systemd transient resource backend is unavailable".to_owned())?;
        let metadata = std::fs::symlink_metadata(&systemd_run)
            .map_err(|_| "systemd transient resource backend is unavailable".to_owned())?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
            || metadata.permissions().mode() & 0o111 == 0
        {
            return Err("systemd transient resource backend failed trust checks".into());
        }
        Ok(Self {
            systemd_run,
            user_manager: nix::unistd::geteuid().as_raw() != 0,
        })
    }

    async fn probe(resources: &ResourceSetupRequest) -> Result<(), String> {
        let backend = Self::discover()?;
        let mut command = backend.base_command();
        let unit = resource_unit_name();
        command
            .arg("--scope")
            .arg("--quiet")
            .arg("--collect")
            .arg(format!("--unit={unit}"));
        for (key, value) in resources.properties() {
            command.arg(format!("--property={key}={value}"));
        }
        command
            .arg("--")
            .arg("/usr/bin/true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|_| "systemd transient resource backend is unavailable".to_owned())?;
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(Ok(status)) if status.success() => Ok(()),
            Ok(_) => Err("systemd could not establish the requested cgroup limits".into()),
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                Err("systemd resource capability probe timed out".into())
            }
        }
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new(&self.systemd_run);
        if self.user_manager {
            command.arg("--user");
        }
        command
    }

    fn wrap(
        &self,
        inner: Command,
        resources: &ResourceSetupRequest,
    ) -> Result<(Command, String), RunnerError> {
        let inner_std = inner.as_std();
        let program = inner_std.get_program().to_owned();
        let args: Vec<_> = inner_std.get_args().map(|arg| arg.to_owned()).collect();
        let cwd = inner_std
            .get_current_dir()
            .ok_or_else(|| RunnerError::Setup("resource command cwd is unavailable".into()))?
            .to_path_buf();
        let environment: Vec<_> = inner_std
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
            .collect();
        let unit = resource_unit_name();
        let mut command = self.base_command();
        command
            .arg("--scope")
            .arg("--quiet")
            .arg(format!("--unit={unit}"))
            .arg("--working-directory")
            .arg(&cwd);
        for (key, value) in resources.properties() {
            command.arg(format!("--property={key}={value}"));
        }
        command.arg("--").arg(program).args(args);
        command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.env_clear();
        for key in ["DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR", "HOME"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.env("PATH", "/usr/bin:/bin");
        for (key, value) in environment {
            command.env(key, value);
        }
        Ok((command, unit))
    }

    async fn unit_result(&self, unit: &str) -> Option<String> {
        let mut command = Command::new(Self::systemctl_path()?);
        self.add_manager_arg(&mut command);
        command
            .arg("show")
            .arg(unit)
            .arg("--property=Result")
            .arg("--value")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(1), command.output())
            .await
            .ok()?
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let result = String::from_utf8(output.stdout).ok()?.trim().to_owned();
        (!result.is_empty()).then_some(result)
    }

    async fn stop_unit(&self, unit: &str) -> Result<(), String> {
        let systemctl = Self::systemctl_path()
            .ok_or_else(|| "systemd cleanup command is unavailable".to_owned())?;
        let mut command = Command::new(systemctl);
        self.add_manager_arg(&mut command);
        command
            .arg("stop")
            .arg(unit)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| "systemd scope cleanup failed".to_owned())?;
        match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
            Ok(Ok(status)) if status.success() => Ok(()),
            Ok(_) => Err("systemd scope cleanup failed".into()),
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                Err("systemd scope cleanup timed out".into())
            }
        }
    }

    fn add_manager_arg(&self, command: &mut Command) {
        if self.user_manager {
            command.arg("--user");
        }
    }

    fn systemctl_path() -> Option<PathBuf> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let systemctl = ["/usr/bin/systemctl", "/bin/systemctl"]
            .iter()
            .find_map(|path| Path::new(path).canonicalize().ok())?;
        let metadata = std::fs::symlink_metadata(&systemctl).ok()?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
            || metadata.permissions().mode() & 0o111 == 0
        {
            return None;
        }
        Some(systemctl)
    }
}

#[cfg(target_os = "linux")]
fn resource_unit_name() -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("eggwork-{}-{nonce}.scope", std::process::id())
}

#[cfg(not(target_os = "linux"))]
struct SystemdCgroupBackend;

#[cfg(not(target_os = "linux"))]
impl SystemdCgroupBackend {
    async fn probe(_resources: &ResourceSetupRequest) -> Result<(), String> {
        Err("OS resource controls are unsupported on this platform".into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceSetupOutcome {
    NotRequested,
    NotApplied { reason: String },
    Applied { backend: String },
    Failed { reason: String },
}

/// Prepare platform enforcement before the child is spawned.
#[async_trait::async_trait]
pub trait ExecutionSetup: Send + Sync {
    async fn prepare(
        &self,
        sandbox: &SandboxRequest,
        resources: &ResourceSetupRequest,
        root: &Path,
    ) -> Result<SetupOutcome, RunnerError>;

    /// Runtime-probed execution capabilities: filesystem isolation and OS
    /// resource controllers. Each returned feature string must reflect a live
    /// probe — never a compile-time or configuration-time guess.
    async fn execution_capabilities(&self) -> Vec<String> {
        Vec::new()
    }

    /// Installation-selected wrapper used when a sandbox request is active.
    fn sandbox_helper_path(&self) -> Option<&Path> {
        None
    }
}

/// Verifies and invokes the installation-owned Landlock sibling helper.
#[derive(Debug, Clone)]
pub struct TrustedLandlockSetup {
    helper_path: PathBuf,
}

impl TrustedLandlockSetup {
    pub fn new(helper_path: impl Into<PathBuf>) -> Self {
        Self {
            helper_path: helper_path.into(),
        }
    }

    pub fn discover_sibling() -> Self {
        let helper_path = std::env::current_exe()
            .ok()
            .and_then(|path| path.canonicalize().ok())
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .map(|parent| parent.join("eggwork-sandbox-helper"))
            .unwrap_or_else(|| PathBuf::from("eggwork-sandbox-helper"));
        Self::new(helper_path)
    }
}

#[async_trait::async_trait]
impl ExecutionSetup for TrustedLandlockSetup {
    async fn prepare(
        &self,
        sandbox: &SandboxRequest,
        resources: &ResourceSetupRequest,
        _root: &Path,
    ) -> Result<SetupOutcome, RunnerError> {
        let mut resource_outcome = if resources.is_requested() {
            match SystemdCgroupBackend::probe(resources).await {
                Ok(()) => ResourceSetupOutcome::Applied {
                    backend: "systemd-cgroup-v2".into(),
                },
                Err(reason) if resources.has_required() => {
                    return Err(RunnerError::Setup(reason));
                }
                Err(reason) => ResourceSetupOutcome::NotApplied { reason },
            }
        } else {
            ResourceSetupOutcome::NotRequested
        };
        if matches!(resource_outcome, ResourceSetupOutcome::Applied { .. })
            && let Err(reason) = verify_trusted_helper(&self.helper_path)
        {
            if resources.has_required() {
                return Err(RunnerError::Setup(reason));
            }
            resource_outcome = ResourceSetupOutcome::NotApplied { reason };
        }
        let outcome = match sandbox {
            SandboxRequest::None => SandboxOutcome::NotRequested,
            SandboxRequest::BestEffort { profile } | SandboxRequest::Required { profile } => {
                if profile != WORKSPACE_RW_PROFILE {
                    let reason = "unsupported sandbox profile".to_owned();
                    if matches!(sandbox, SandboxRequest::BestEffort { .. }) {
                        return Ok(SetupOutcome {
                            sandbox: SandboxOutcome::NotApplied { reason },
                            resources: resource_outcome,
                        });
                    }
                    return Err(RunnerError::Setup(reason));
                }
                match verify_trusted_helper(&self.helper_path) {
                    Ok(()) => SandboxOutcome::Applied {
                        profile: profile.clone(),
                    },
                    Err(reason) if matches!(sandbox, SandboxRequest::BestEffort { .. }) => {
                        SandboxOutcome::NotApplied { reason }
                    }
                    Err(reason) => return Err(RunnerError::Setup(reason)),
                }
            }
        };
        Ok(SetupOutcome {
            sandbox: outcome,
            resources: resource_outcome,
        })
    }

    async fn execution_capabilities(&self) -> Vec<String> {
        let mut capabilities = Vec::new();
        if verify_trusted_helper(&self.helper_path).is_ok() && probe_landlock_ruleset() {
            capabilities.push(LANDLOCK_WORKSPACE_RW_CAPABILITY.into());
        }
        for (feature, request) in CGROUP_PROBE_TABLE {
            if SystemdCgroupBackend::probe(&request).await.is_ok() {
                capabilities.push(feature.into());
            }
        }
        capabilities
    }

    fn sandbox_helper_path(&self) -> Option<&Path> {
        Some(&self.helper_path)
    }
}

/// Public capability feature name advertised for the closed Landlock
/// `workspace_rw` profile used by `IsolationRequirement::{BestEffort, Required}`.
pub const LANDLOCK_WORKSPACE_RW_CAPABILITY: &str = "isolation.landlock.workspace-rw.v1";

/// Public capability feature name advertised when the cgroup v2 memory
/// controller is enforced.
///
/// The value is the frozen public feature name for the memory dimension and
/// must not change without an ADR: a node advertises it and a server admits
/// a required memory dimension only when the advertised name matches, so a
/// renamed byte would turn every memory-limited request into a
/// capability refusal.
pub const CGROUPS_V2_MEMORY_CAPABILITY: &str = "resources.cgroups-v2.memory";

/// Public capability feature name advertised when the cgroup v2 CPU
/// controller is enforced.
///
/// The value is the frozen public feature name for the CPU dimension and must
/// not change without an ADR, for the same reason as
/// [`CGROUPS_V2_MEMORY_CAPABILITY`].
pub const CGROUPS_V2_CPU_CAPABILITY: &str = "resources.cgroups-v2.cpu";

/// Public capability feature name advertised when the cgroup v2 pids
/// controller is enforced.
///
/// The value is the frozen public feature name for the pids dimension and must
/// not change without an ADR, for the same reason as
/// [`CGROUPS_V2_MEMORY_CAPABILITY`].
pub const CGROUPS_V2_PIDS_CAPABILITY: &str = "resources.cgroups-v2.pids";

/// The only sandbox profile the protocol can request.
///
/// The standalone helper pins its spelling in a parity test. Renaming it
/// requires an ADR because mismatched names fail closed.
pub const WORKSPACE_RW_PROFILE: &str = "workspace_rw";

/// The cgroup v2 probes the runner runs before it advertises a resource
/// feature, paired with the feature name each probe earns.
///
/// The table is a `const` so a test can read the names it would advertise
/// without running the probes: the names here are the exported constants, and
/// a literal typed into the table would show up as a name that is not one of
/// them.
const CGROUP_PROBE_TABLE: [(&str, ResourceSetupRequest); 3] = [
    (
        CGROUPS_V2_MEMORY_CAPABILITY,
        ResourceSetupRequest {
            memory_bytes: Requirement::BestEffort(32 * 1024 * 1024),
            cpu_millis: Requirement::NotRequested,
            pids: Requirement::NotRequested,
        },
    ),
    (
        CGROUPS_V2_CPU_CAPABILITY,
        ResourceSetupRequest {
            memory_bytes: Requirement::NotRequested,
            cpu_millis: Requirement::BestEffort(100),
            pids: Requirement::NotRequested,
        },
    ),
    (
        CGROUPS_V2_PIDS_CAPABILITY,
        ResourceSetupRequest {
            memory_bytes: Requirement::NotRequested,
            cpu_millis: Requirement::NotRequested,
            pids: Requirement::BestEffort(16),
        },
    ),
];

/// Probe whether Landlock can create the helper's `workspace_rw` ruleset.
///
/// The probe adds the same handles and rights without applying restrictions
/// to this process; any unsupported path or right returns `false`.
#[cfg(target_os = "linux")]
fn probe_landlock_ruleset() -> bool {
    use landlock::{
        ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    };

    let abi = ABI::V4;
    let handled = AccessFs::from_all(abi);
    let ruleset = match Ruleset::default().handle_access(handled) {
        Ok(ruleset) => ruleset,
        Err(_) => return false,
    };
    let mut ruleset = match ruleset.create() {
        Ok(ruleset) => ruleset,
        Err(_) => return false,
    };
    for path_str in ["/usr", "/etc/ld.so.cache", "/etc/ssl/certs", "/dev/null"] {
        let path = Path::new(path_str);
        let canonical = match path.canonicalize() {
            Ok(canonical) => canonical,
            Err(_) => continue,
        };
        let path_fd = match PathFd::new(&canonical) {
            Ok(fd) => fd,
            Err(_) => return false,
        };
        let access = if canonical.is_dir() {
            AccessFs::from_read(abi) | AccessFs::Execute
        } else if canonical == Path::new("/dev/null") {
            // Must mirror the helper's null-device rule exactly: the
            // capability probe proves the kernel supports the precise
            // rights the sandbox will apply.
            landlock::make_bitflags!(AccessFs::{ReadFile | WriteFile})
        } else {
            landlock::make_bitflags!(AccessFs::{ReadFile})
        };
        ruleset = match ruleset.add_rule(PathBeneath::new(path_fd, access)) {
            Ok(ruleset) => ruleset,
            Err(_) => return false,
        };
    }
    true
}

#[cfg(not(target_os = "linux"))]
fn probe_landlock_ruleset() -> bool {
    false
}

/// Check the helper trust boundary used by execution, admission, and diagnostics.
///
/// Required isolation fails closed when this check returns `Err`.
#[cfg(target_os = "linux")]
pub fn verify_trusted_helper(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "sandbox helper is unavailable".to_owned())?;
    let mode = metadata.permissions().mode();
    let effective_uid = nix::unistd::geteuid().as_raw();
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || (metadata.uid() != 0 && metadata.uid() != effective_uid)
        || mode & 0o022 != 0
        || mode & 0o111 == 0
    {
        return Err("sandbox helper trust check failed".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "sandbox helper location is invalid".to_owned())?
        .canonicalize()
        .map_err(|_| "sandbox helper location is invalid".to_owned())?;
    let parent_metadata =
        std::fs::metadata(&parent).map_err(|_| "sandbox helper location is invalid".to_owned())?;
    if !parent_metadata.is_dir()
        || (parent_metadata.uid() != 0 && parent_metadata.uid() != effective_uid)
        || parent_metadata.permissions().mode() & 0o022 != 0
    {
        return Err("sandbox helper directory trust check failed".into());
    }
    let mut ancestor = parent.as_path();
    while let Some(next) = ancestor.parent() {
        let metadata =
            std::fs::metadata(next).map_err(|_| "sandbox helper ancestor is invalid".to_owned())?;
        let mode = metadata.permissions().mode();
        let sticky_root_directory = metadata.is_dir() && metadata.uid() == 0 && mode & 0o1000 != 0;
        if !metadata.is_dir()
            || ((metadata.uid() != 0 && metadata.uid() != effective_uid) && !sticky_root_directory)
            || (mode & 0o022 != 0 && !sticky_root_directory)
        {
            return Err("sandbox helper ancestor trust check failed".into());
        }
        ancestor = next;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn verify_trusted_helper(_path: &Path) -> Result<(), String> {
    Err("trusted Landlock is unsupported on this platform".into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOutcome {
    pub sandbox: SandboxOutcome,
    pub resources: ResourceSetupOutcome,
}

#[derive(Debug, Default)]
pub struct NoExecutionSetup;

#[async_trait::async_trait]
impl ExecutionSetup for NoExecutionSetup {
    async fn prepare(
        &self,
        sandbox: &SandboxRequest,
        resources: &ResourceSetupRequest,
        _root: &Path,
    ) -> Result<SetupOutcome, RunnerError> {
        if matches!(sandbox, SandboxRequest::Required { .. }) || resources.has_required() {
            return Err(RunnerError::RequiredSetupUnavailable);
        }
        Ok(SetupOutcome {
            sandbox: match sandbox {
                SandboxRequest::None => SandboxOutcome::NotRequested,
                SandboxRequest::BestEffort { .. } | SandboxRequest::Required { .. } => {
                    SandboxOutcome::NotApplied {
                        reason: "no sandbox backend is configured".into(),
                    }
                }
            },
            resources: if resources.is_requested() {
                ResourceSetupOutcome::NotApplied {
                    reason: "no resource-control backend is configured".into(),
                }
            } else {
                ResourceSetupOutcome::NotRequested
            },
        })
    }
}

/// The `requirement` value a degradation diagnostic always carries.
///
/// A degradation is by definition a *best-effort* request that was not
/// honoured: a required request is refused instead, and a refusal is not a
/// degradation.
const DIAGNOSTIC_REQUIREMENT_BEST_EFFORT: &str = "best_effort";

/// The degradations the runner has already reported to the caller.
///
/// The bookkeeping exists so that one degraded dimension produces exactly one
/// diagnostic event. A degradation can be discovered more than once — a
/// sandbox that `prepare` could not apply may also be the sandbox the helper
/// handshake could not enforce — and the second discovery must refine the
/// recorded outcome, not duplicate the event.
#[derive(Debug, Default)]
struct ReportedDegradations {
    isolation: bool,
    resources: Vec<eggwork_core::ResourceDimension>,
}

/// The best-effort resource dimensions a request asked for.
///
/// `Required` dimensions are excluded on purpose. A required dimension that
/// reached a `NotApplied` outcome would be a failure of the fail-closed rule,
/// not a degradation, and reporting it as one would turn a refusal into an
/// advisory. The in-tree setups refuse a required request before they can
/// produce such an outcome, so this filter never drops a real report.
fn best_effort_resource_dimensions(
    request: &ResourceSetupRequest,
) -> Vec<eggwork_core::ResourceDimension> {
    let mut dimensions = Vec::new();
    if matches!(request.memory_bytes, Requirement::BestEffort(_)) {
        dimensions.push(eggwork_core::ResourceDimension::Memory);
    }
    if matches!(request.cpu_millis, Requirement::BestEffort(_)) {
        dimensions.push(eggwork_core::ResourceDimension::Cpu);
    }
    if matches!(request.pids, Requirement::BestEffort(_)) {
        dimensions.push(eggwork_core::ResourceDimension::Pids);
    }
    dimensions
}

/// The wire name of a resource dimension, as the diagnostic grammar spells it.
fn resource_dimension_name(dimension: eggwork_core::ResourceDimension) -> &'static str {
    match dimension {
        eggwork_core::ResourceDimension::Memory => "memory",
        eggwork_core::ResourceDimension::Cpu => "cpu",
        eggwork_core::ResourceDimension::Pids => "pids",
    }
}

/// The requested dimensions whose post-mortem counter the helper could not
/// open.
///
/// Only the dimensions the helper actually reported as unobservable are named:
/// the diagnostic has to say which limit could not be *verified*, not merely
/// that the resource outcome was downgraded.
#[cfg(target_os = "linux")]
fn unobserved_resource_dimensions(
    request: &ResourceSetupRequest,
    unobserved: &[eggwork_core::ResourceDimension],
) -> Vec<eggwork_core::ResourceDimension> {
    let mut dimensions = Vec::new();
    if matches!(request.memory_bytes, Requirement::BestEffort(_))
        && unobserved.contains(&eggwork_core::ResourceDimension::Memory)
    {
        dimensions.push(eggwork_core::ResourceDimension::Memory);
    }
    if matches!(request.pids, Requirement::BestEffort(_))
        && unobserved.contains(&eggwork_core::ResourceDimension::Pids)
    {
        dimensions.push(eggwork_core::ResourceDimension::Pids);
    }
    dimensions
}

/// Builds the diagnostic payload for a degraded filesystem isolation.
///
/// The structured keys come first and the free-form reason last, so a reason
/// that does not fit absorbs the truncation and the structured keys survive
/// intact.
fn isolation_diagnostic(profile: &str, reason: &str) -> Option<String> {
    let mut payload =
        eggwork_core::DiagnosticPayload::new(eggwork_core::DIAGNOSTIC_CLASS_ISOLATION_DEGRADED)
            .ok()?;
    let _ = payload.set("requirement", DIAGNOSTIC_REQUIREMENT_BEST_EFFORT);
    let _ = payload.set("profile", profile);
    let _ = payload.set("reason", reason);
    Some(payload.build())
}

/// Builds the diagnostic payload for a degraded resource dimension.
fn resources_diagnostic(
    dimension: eggwork_core::ResourceDimension,
    reason: &str,
) -> Option<String> {
    let mut payload =
        eggwork_core::DiagnosticPayload::new(eggwork_core::DIAGNOSTIC_CLASS_RESOURCES_DEGRADED)
            .ok()?;
    let _ = payload.set("requirement", DIAGNOSTIC_REQUIREMENT_BEST_EFFORT);
    let _ = payload.set("dimension", resource_dimension_name(dimension));
    let _ = payload.set("reason", reason);
    Some(payload.build())
}

/// Sends one diagnostic payload on the runner's output channel.
///
/// The channel is the only path from the runner to the caller that preserves
/// ordering: the server drains it in order, so a payload sent before the
/// target is spawned is published before anything the target writes.
async fn send_diagnostic(output_tx: &mpsc::Sender<OutputChunk>, payload: Option<String>) {
    let Some(payload) = payload else {
        // The payload could not be built, which can only mean the class is not
        // in the closed set. There is no grammar-conforming payload to send,
        // and a malformed one would be worse than none: the degradation stays
        // recorded in the terminal snapshot.
        return;
    };
    // Await the bounded channel so a degradation diagnostic is never silently
    // dropped; backpressure is preferable to losing the event.
    let _ = output_tx.send(OutputChunk::Diagnostic(payload)).await;
}

/// Emits the diagnostic events for the degradations `setup` records.
///
/// Only degradations of *best-effort* requests are reported: a required
/// request that could not be satisfied is refused before the target starts
/// and is never described as a degradation.
async fn report_setup_degradations(
    output_tx: &mpsc::Sender<OutputChunk>,
    sandbox_request: &SandboxRequest,
    resource_request: &ResourceSetupRequest,
    setup: &SetupOutcome,
    reported: &mut ReportedDegradations,
) {
    if !reported.isolation
        && let SandboxOutcome::NotApplied { reason } = &setup.sandbox
        && let SandboxRequest::BestEffort { profile } = sandbox_request
    {
        reported.isolation = true;
        send_diagnostic(output_tx, isolation_diagnostic(profile, reason)).await;
    }
    if let ResourceSetupOutcome::NotApplied { reason } = &setup.resources {
        for dimension in best_effort_resource_dimensions(resource_request) {
            if reported.resources.contains(&dimension) {
                continue;
            }
            reported.resources.push(dimension);
            send_diagnostic(output_tx, resources_diagnostic(dimension, reason)).await;
        }
    }
}

#[derive(Clone)]
pub struct RunnerRequest {
    argv: Vec<String>,
    root: PathBuf,
    cwd: Option<RelativePath>,
    environment: Vec<(String, String)>,
    stdin: StdinPolicy,
    timeout: Duration,
    capture_limit: usize,
    event_chunk_bytes: usize,
    overflow: CoreOverflowPolicy,
    provenance: ExecutionProvenance,
    sandbox: SandboxRequest,
    resources: ResourceSetupRequest,
}

impl std::fmt::Debug for RunnerRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerRequest")
            .field("argv", &"[REDACTED]")
            .field("root", &self.root)
            .field("cwd", &self.cwd)
            .field("environment_count", &self.environment.len())
            .field("stdin", &self.stdin)
            .field("timeout", &self.timeout)
            .field("capture_limit", &self.capture_limit)
            .field("event_chunk_bytes", &self.event_chunk_bytes)
            .field("overflow", &self.overflow)
            .field("provenance", &self.provenance)
            .field("sandbox", &self.sandbox)
            .field("resources", &self.resources)
            .finish()
    }
}

impl RunnerRequest {
    /// Create an embedding request with conservative defaults.
    ///
    /// The request is covered by the `eggwork_core` domain contract like every
    /// other construction path: [`RunnerRequest::contract_error`] reports a
    /// violation without running anything, and [`run`](LocalProcessRunner::run)
    /// refuses one before setup and before any child creation.
    pub fn new(argv: Vec<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            argv,
            root: root.into(),
            cwd: None,
            environment: Vec::new(),
            stdin: StdinPolicy::Null,
            timeout: Duration::from_secs(30),
            capture_limit: MAX_CAPTURE_BYTES,
            event_chunk_bytes: MAX_EVENT_CHUNK_BYTES,
            overflow: CoreOverflowPolicy::Truncate,
            provenance: ExecutionProvenance::default(),
            sandbox: SandboxRequest::None,
            resources: ResourceSetupRequest {
                memory_bytes: Requirement::NotRequested,
                cpu_millis: Requirement::NotRequested,
                pids: Requirement::NotRequested,
            },
        }
    }

    pub fn set_argv(&mut self, argv: Vec<String>) {
        self.argv = argv;
    }

    pub fn set_working_directory(&mut self, cwd: Option<RelativePath>) {
        self.cwd = cwd;
    }

    /// Replace the request's environment.
    ///
    /// The entries are the caller's raw name/value pairs; the environment-entry
    /// rules themselves are owned by `eggwork_core` and are applied by
    /// [`RunnerRequest::contract_error`] and by
    /// [`run`](LocalProcessRunner::run), not restated here.
    pub fn set_environment(&mut self, environment: Vec<(String, String)>) {
        self.environment = environment;
    }

    pub fn set_stdin_policy(&mut self, stdin: StdinPolicy) {
        self.stdin = stdin;
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn set_output_policy(
        &mut self,
        capture_limit: usize,
        event_chunk_bytes: usize,
        overflow: CoreOverflowPolicy,
    ) {
        self.capture_limit = capture_limit;
        self.event_chunk_bytes = event_chunk_bytes;
        self.overflow = overflow;
    }

    pub fn set_provenance(&mut self, provenance: ExecutionProvenance) {
        self.provenance = provenance;
    }

    pub fn set_sandbox_request(&mut self, sandbox: SandboxRequest) {
        self.sandbox = sandbox;
    }

    pub fn set_resource_setup_request(&mut self, resources: ResourceSetupRequest) {
        self.resources = resources;
    }

    pub fn resource_setup_request_mut(&mut self) -> &mut ResourceSetupRequest {
        &mut self.resources
    }

    /// Return the requested sandbox policy without exposing request internals.
    pub fn sandbox_request(&self) -> &SandboxRequest {
        &self.sandbox
    }

    /// Return requested resource controls without exposing request internals.
    pub fn resource_setup_request(&self) -> &ResourceSetupRequest {
        &self.resources
    }

    /// Translate a validated protocol-neutral command to a local request.
    pub fn from_spec(
        spec: &ExecutionSpec,
        root: impl Into<PathBuf>,
        provenance: ExecutionProvenance,
    ) -> Result<Self, RunnerError> {
        spec.validate().map_err(RunnerError::InvalidRequest)?;
        let CommandSpec {
            argv,
            cwd,
            environment,
            stdin,
            timeout_millis,
            output,
            resources,
            isolation,
            ..
        } = spec.command.clone();
        let sandbox = match isolation {
            eggwork_core::IsolationRequirement::None => SandboxRequest::None,
            eggwork_core::IsolationRequirement::BestEffort => SandboxRequest::BestEffort {
                profile: WORKSPACE_RW_PROFILE.into(),
            },
            eggwork_core::IsolationRequirement::Required => SandboxRequest::Required {
                profile: WORKSPACE_RW_PROFILE.into(),
            },
        };
        Ok(Self {
            argv,
            root: root.into(),
            cwd,
            environment: environment.into_iter().map(|e| (e.name, e.value)).collect(),
            stdin,
            timeout: Duration::from_millis(timeout_millis),
            capture_limit: output.capture_limit_bytes,
            event_chunk_bytes: output.event_chunk_bytes,
            overflow: output.overflow,
            provenance,
            sandbox,
            resources: ResourceSetupRequest {
                memory_bytes: resources.memory_bytes,
                cpu_millis: resources.cpu_millis,
                pids: resources.pids,
            },
        })
    }

    /// Return the first request-contract violation without touching the filesystem.
    ///
    /// `run` uses the same check before setup or spawn. Root and working
    /// directory checks are separate because they require filesystem access.
    pub fn contract_error(&self) -> Option<ValidationError> {
        self.contract_violation()
    }

    /// The request's environment as core environment entries.
    ///
    /// The runner stores raw name/value pairs because that is what the child's
    /// environment is built from, but the *rules* that govern an entry are
    /// owned by `eggwork_core`. The entries are handed over as plain values so
    /// that `eggwork_core` re-validates them rather than trusting them.
    fn environment_entries(&self) -> Vec<EnvironmentEntry> {
        self.environment
            .iter()
            .map(|(name, value)| EnvironmentEntry {
                name: name.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// The request's output policy as the core output policy.
    fn output_policy(&self) -> OutputPolicy {
        OutputPolicy {
            capture_limit_bytes: self.capture_limit,
            event_chunk_bytes: self.event_chunk_bytes,
            overflow: self.overflow.clone(),
        }
    }

    /// The request's resource requirements as the core resource requirements.
    fn resource_requirements(&self) -> ResourceRequirements {
        ResourceRequirements {
            memory_bytes: self.resources.memory_bytes,
            cpu_millis: self.resources.cpu_millis,
            pids: self.resources.pids,
        }
    }

    /// Return the first violation in core's validation order, with the runner's
    /// stricter argv-entry check immediately after argv bounds.
    fn contract_violation(&self) -> Option<ValidationError> {
        validate_argv(&self.argv)
            .err()
            .or_else(|| self.argv_entry_violation())
            .or_else(|| validate_environment_entries(&self.environment_entries()).err())
            .or_else(|| validate_timeout(self.timeout).err())
            .or_else(|| self.stdin.validate().err())
            .or_else(|| self.output_policy().validate().err())
            .or_else(|| self.resource_requirements().validate().err())
    }

    /// Reject empty arguments, which core permits but process construction does not.
    fn argv_entry_violation(&self) -> Option<ValidationError> {
        self.argv
            .iter()
            .any(|argument| argument.is_empty())
            .then_some(ValidationError::InvalidSyntax {
                field: "argv entry",
            })
    }

    fn validate(&self) -> Result<PathBuf, RunnerError> {
        // Every core-owned rule is consumed from `eggwork_core` rather than
        // restated here, so the runner cannot drift from the contract it
        // claims to enforce.
        if let Some(violation) = self.contract_violation() {
            return Err(RunnerError::InvalidRequest(violation));
        }
        // The runner's own contract rules: the root-directory and
        // working-directory resolution. `eggwork_core` does not own them,
        // because they are about a filesystem the domain layer never sees.
        let root = self
            .root
            .canonicalize()
            .map_err(|e| RunnerError::InvalidRoot(e.to_string()))?;
        if !root.is_dir() {
            return Err(RunnerError::InvalidRoot(
                "execution root is not a directory".into(),
            ));
        }
        let cwd = if let Some(relative) = &self.cwd {
            let path = root.join(relative.as_str());
            let canonical = path
                .canonicalize()
                .map_err(|e| RunnerError::InvalidWorkingDirectory(e.to_string()))?;
            if !canonical.starts_with(&root) || !canonical.is_dir() {
                return Err(RunnerError::InvalidWorkingDirectory(
                    "cwd escapes root or is not a directory".into(),
                ));
            }
            canonical
        } else {
            root.clone()
        };
        Ok(cwd)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationReason {
    Exited,
    TimedOut,
    Cancelled,
    OutputLimit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupDiagnostics {
    pub process_group_signal_error: Option<String>,
    pub wait_error: Option<String>,
    pub stdin_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerResult {
    pub termination: TerminationReason,
    pub exit_code: Option<i32>,
    pub stdout: BoundedCapture,
    pub stderr: BoundedCapture,
    pub cleanup: CleanupDiagnostics,
    pub setup: SetupOutcome,
    pub resource_request: ResourceSetupRequest,
    pub resource_limits_exceeded: Vec<eggwork_core::ResourceDimension>,
    pub stream_chunks_dropped: u64,
    pub provenance: ExecutionProvenance,
}

impl RunnerResult {
    pub fn execution_result(&self) -> ExecutionResult {
        let state = if !self.resource_limits_exceeded.is_empty() {
            ExecutionState::Failed
        } else {
            match self.termination {
                TerminationReason::Exited if self.exit_code == Some(0) => ExecutionState::Succeeded,
                TerminationReason::Exited => ExecutionState::Failed,
                TerminationReason::TimedOut => ExecutionState::TimedOut,
                TerminationReason::Cancelled => ExecutionState::Cancelled,
                TerminationReason::OutputLimit => ExecutionState::Failed,
            }
        };
        let failure = if !self.resource_limits_exceeded.is_empty() {
            Some(ExecutionFailure::ResourceLimit)
        } else {
            match self.termination {
                TerminationReason::OutputLimit => Some(ExecutionFailure::OutputLimit),
                _ if self.exit_code.is_none()
                    && matches!(self.termination, TerminationReason::Exited) =>
                {
                    Some(ExecutionFailure::Internal)
                }
                _ => None,
            }
        };
        ExecutionResult {
            state,
            exit_code: self.exit_code,
            failure,
            stdout_bytes: self.stdout.total_bytes,
            stderr_bytes: self.stderr.total_bytes,
            stdout_omitted: self.stdout.omitted_bytes,
            stderr_omitted: self.stderr.omitted_bytes,
            cleanup_warning: self
                .cleanup
                .process_group_signal_error
                .clone()
                .or_else(|| self.cleanup.wait_error.clone())
                .or_else(|| self.cleanup.stdin_error.clone()),
            finalization_failure: None,
            artifact_count: 0,
            // The runner owns the process, not the capture: the server's
            // finalization decides the capture disposition and replaces this
            // placeholder. The placeholder claims nothing.
            artifact_capture: eggwork_core::ArtifactCaptureDisposition::Unknown,
            sandbox: Some(match &self.setup.sandbox {
                SandboxOutcome::NotRequested => eggwork_core::SandboxResult::NotRequested,
                SandboxOutcome::NotApplied { reason } => eggwork_core::SandboxResult::NotApplied {
                    reason: reason.clone(),
                },
                SandboxOutcome::Applied { profile } => eggwork_core::SandboxResult::Applied {
                    profile: profile.clone(),
                },
                SandboxOutcome::Failed { reason } => eggwork_core::SandboxResult::Failed {
                    reason: reason.clone(),
                },
            }),
            resources: Some(resource_result(
                &self.setup.resources,
                &self.resource_request,
                &self.resource_limits_exceeded,
            )),
        }
    }
}

fn resource_result(
    outcome: &ResourceSetupOutcome,
    request: &ResourceSetupRequest,
    limits_exceeded: &[eggwork_core::ResourceDimension],
) -> eggwork_core::ResourceResult {
    use eggwork_core::ResourceDimensionResult as Dimension;
    let dimension = |requested: bool, kind| match outcome {
        ResourceSetupOutcome::NotRequested if !requested => Dimension::NotRequested,
        ResourceSetupOutcome::NotRequested => Dimension::NotApplied {
            reason: "resource control was not requested".into(),
        },
        ResourceSetupOutcome::NotApplied { reason } | ResourceSetupOutcome::Failed { reason }
            if requested =>
        {
            Dimension::NotApplied {
                reason: reason.clone(),
            }
        }
        ResourceSetupOutcome::Applied { backend } if requested => {
            if limits_exceeded.contains(&kind) {
                Dimension::LimitExceeded {
                    backend: backend.clone(),
                }
            } else {
                Dimension::Applied {
                    backend: backend.clone(),
                }
            }
        }
        _ => Dimension::NotRequested,
    };
    eggwork_core::ResourceResult {
        memory_bytes: dimension(
            !matches!(request.memory_bytes, Requirement::NotRequested),
            eggwork_core::ResourceDimension::Memory,
        ),
        cpu_millis: dimension(
            !matches!(request.cpu_millis, Requirement::NotRequested),
            eggwork_core::ResourceDimension::Cpu,
        ),
        pids: dimension(
            !matches!(request.pids, Requirement::NotRequested),
            eggwork_core::ResourceDimension::Pids,
        ),
    }
}

#[derive(Debug, Error)]
pub enum RunnerError {
    /// The request violates the `eggwork_core` domain contract.
    ///
    /// The payload is the core [`ValidationError`], so the failure stays
    /// attributable to the rule that produced it: an environment name with
    /// invalid syntax and a duplicate environment name are two different
    /// rules, and a caller can tell them apart without parsing a message.
    #[error("invalid execution request: {0}")]
    InvalidRequest(ValidationError),
    #[error("execution root is invalid: {0}")]
    InvalidRoot(String),
    #[error("working directory is invalid: {0}")]
    InvalidWorkingDirectory(String),
    #[error("execution was cancelled before spawn")]
    CancelledBeforeSpawn,
    #[error("sandbox or resource setup failed: {0}")]
    Setup(String),
    #[error("required sandbox/resource setup has no configured backend")]
    RequiredSetupUnavailable,
    #[error("process-tree control is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("process spawn failed: {0}")]
    Spawn(String),
    #[error("process I/O failed: {0}")]
    Io(String),
    #[error("process wait failed: {0}")]
    Wait(String),
}

/// The single asynchronous finite-process execution service.
pub struct LocalProcessRunner {
    setup: Box<dyn ExecutionSetup>,
}

impl Default for LocalProcessRunner {
    fn default() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::new(TrustedLandlockSetup::discover_sibling())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::new(NoExecutionSetup)
        }
    }
}

impl LocalProcessRunner {
    pub fn new(setup: impl ExecutionSetup + 'static) -> Self {
        Self {
            setup: Box::new(setup),
        }
    }

    /// Returns the runtime-probed execution capability feature strings
    /// contributed by the configured `ExecutionSetup`. Callers MUST combine
    /// these with the static protocol/workspace/artifact feature list; the
    /// runner owns dynamic facts only.
    pub async fn execution_capabilities(&self) -> Vec<String> {
        self.setup.execution_capabilities().await
    }

    pub async fn run(
        &self,
        request: RunnerRequest,
        cancellation: CancellationToken,
        output_tx: mpsc::Sender<OutputChunk>,
    ) -> Result<RunnerResult, RunnerError> {
        if cancellation.is_cancelled() {
            return Err(RunnerError::CancelledBeforeSpawn);
        }
        let cwd = request.validate()?;
        // Process-tree ownership is platform-specific; request preparation and
        // result handling stay shared. Unsupported targets refuse before spawn.
        // Only the Linux helper handshake updates the prepared setup outcome.
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut setup = self
            .setup
            .prepare(&request.sandbox, &request.resources, &request.root)
            .await?;
        // The degradations `prepare` decided are known before any process
        // exists, so their diagnostics are emitted here — before the target is
        // spawned, and therefore before the target can produce any output. A
        // diagnostic the server derived from the runner's result instead would
        // necessarily arrive after all target output, which is the gap this
        // emission point exists to close.
        let mut reported_degradations = ReportedDegradations::default();
        report_setup_degradations(
            &output_tx,
            &request.sandbox,
            &request.resources,
            &setup,
            &mut reported_degradations,
        )
        .await;
        #[cfg(target_os = "linux")]
        let use_helper = ((!matches!(request.sandbox, SandboxRequest::None)
            && matches!(setup.sandbox, SandboxOutcome::Applied { .. }))
            || matches!(setup.resources, ResourceSetupOutcome::Applied { .. }))
            && self.setup.sandbox_helper_path().is_some();
        #[cfg(target_os = "linux")]
        let mut channel = if use_helper {
            match SandboxChannel::new(
                self.setup.sandbox_helper_path().expect("checked above"),
                &request,
                cwd.clone(),
                matches!(setup.resources, ResourceSetupOutcome::Applied { .. }),
            ) {
                Ok(channel) => Some(channel),
                Err(error)
                    if !matches!(request.sandbox, SandboxRequest::Required { .. })
                        && !request.resources.has_required() =>
                {
                    if !matches!(request.sandbox, SandboxRequest::None) {
                        setup.sandbox = SandboxOutcome::NotApplied {
                            reason: error.to_string(),
                        };
                    }
                    if matches!(setup.resources, ResourceSetupOutcome::Applied { .. }) {
                        setup.resources = ResourceSetupOutcome::NotApplied {
                            reason: error.to_string(),
                        };
                    }
                    // The helper could not even be launched, so the target is
                    // about to run without the requested enforcement. The
                    // diagnostic is emitted here, before the target exists.
                    report_setup_degradations(
                        &output_tx,
                        &request.sandbox,
                        &request.resources,
                        &setup,
                        &mut reported_degradations,
                    )
                    .await;
                    None
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        #[cfg(not(target_os = "linux"))]
        let _channel: Option<()> = None;
        #[cfg(target_os = "linux")]
        let command = if let Some(channel) = channel.as_ref() {
            channel.command()
        } else {
            direct_command(&request, cwd.clone())
        };
        #[cfg(not(target_os = "linux"))]
        let command = direct_command(&request, cwd.clone());
        #[cfg(target_os = "linux")]
        let (command, mut resource_unit) =
            wrap_resource_command(command, &request.resources, &setup.resources)?;
        #[cfg(target_os = "linux")]
        let mut sandbox_session = None;
        #[cfg(not(target_os = "linux"))]
        let _resource_unit: Option<String> = None;
        // On Linux this is the sandbox helper when isolation or resource
        // enforcement is configured, and the target itself otherwise. On
        // Windows there is no helper, so it is always the target. Either way
        // the tree owner holds the whole tree for the rest of the lifecycle.
        let mut tree = ProcessTree::spawn_platform(command)?;
        #[cfg(target_os = "linux")]
        if let Some(sandbox_channel) = channel.take() {
            let handshake = sandbox_channel
                .wait(tree.child_mut(), cancellation.clone())
                .await;
            match handshake {
                Ok(Some(session)) => sandbox_session = Some(session),
                Ok(None)
                    if !matches!(request.sandbox, SandboxRequest::Required { .. })
                        && !request.resources.has_required() =>
                {
                    let _ = tree.wait().await;
                    if !matches!(request.sandbox, SandboxRequest::None) {
                        setup.sandbox = SandboxOutcome::NotApplied {
                            reason: "Landlock helper could not enforce the requested profile"
                                .into(),
                        };
                    }
                    if matches!(setup.resources, ResourceSetupOutcome::Applied { .. }) {
                        setup.resources = ResourceSetupOutcome::NotApplied {
                            reason: "resource helper could not verify the cgroup limits".into(),
                        };
                    }
                    // The handshake failure is classified and the outcomes are
                    // rewritten, and only now is the degradation reported: the
                    // target is about to be re-spawned directly, with no
                    // Landlock ruleset and no cgroup verification, so the
                    // caller has to learn that before the target runs.
                    report_setup_degradations(
                        &output_tx,
                        &request.sandbox,
                        &request.resources,
                        &setup,
                        &mut reported_degradations,
                    )
                    .await;
                    let direct = direct_command(&request, cwd.clone());
                    let (direct, unit) =
                        wrap_resource_command(direct, &request.resources, &setup.resources)?;
                    resource_unit = unit;
                    tree = ProcessTree::spawn_platform(direct)?;
                }
                Ok(None) => {
                    let _ = tree.start_kill();
                    let _ = tree.wait().await;
                    return Err(RunnerError::RequiredSetupUnavailable);
                }
                Err(SandboxWaitError::BeforeTarget(error))
                    if !matches!(request.sandbox, SandboxRequest::Required { .. })
                        && !request.resources.has_required()
                        && !matches!(error, RunnerError::CancelledBeforeSpawn) =>
                {
                    let _ = tree.start_kill();
                    let _ = tree.wait().await;
                    if !matches!(request.sandbox, SandboxRequest::None) {
                        setup.sandbox = SandboxOutcome::NotApplied {
                            reason: error.to_string(),
                        };
                    }
                    if matches!(setup.resources, ResourceSetupOutcome::Applied { .. }) {
                        setup.resources = ResourceSetupOutcome::NotApplied {
                            reason: error.to_string(),
                        };
                    }
                    // Same rule as the `Ok(None)` arm above: the degradation is
                    // reported after the handshake failure is classified and
                    // before the target is re-spawned directly.
                    report_setup_degradations(
                        &output_tx,
                        &request.sandbox,
                        &request.resources,
                        &setup,
                        &mut reported_degradations,
                    )
                    .await;
                    let direct = direct_command(&request, cwd.clone());
                    let (direct, unit) =
                        wrap_resource_command(direct, &request.resources, &setup.resources)?;
                    resource_unit = unit;
                    tree = ProcessTree::spawn_platform(direct)?;
                }
                Err(error) => {
                    let _ = tree.start_kill();
                    let _ = tree.wait().await;
                    return Err(error.into_runner_error());
                }
            }
        }
        let stdin_task = match (tree.take_stdin(), &request.stdin) {
            (Some(mut stdin), StdinPolicy::Bytes(bytes)) => {
                let bytes = bytes.clone();
                Some(tokio::spawn(async move {
                    let result = stdin.write_all(&bytes).await;
                    drop(stdin);
                    result
                }))
            }
            (stdin, StdinPolicy::Null) => {
                drop(stdin);
                None
            }
            (stdin, StdinPolicy::Bytes(_)) => {
                drop(stdin);
                None
            }
        };
        let (overflow_tx, mut overflow_rx) = watch::channel(false);
        let stdout = tree
            .take_stdout()
            .ok_or_else(|| RunnerError::Io("stdout pipe missing".into()))?;
        let stderr = tree
            .take_stderr()
            .ok_or_else(|| RunnerError::Io("stderr pipe missing".into()))?;
        // A post-mortem resource observation is only knowable after the target
        // exited, so the diagnostic that reports it needs a sender of its own:
        // `output_tx` is moved into the readers below. The clone is dropped
        // again as soon as the observation has been reported, so the output
        // channel still closes exactly when the readers finish.
        #[cfg(target_os = "linux")]
        let diagnostic_tx = output_tx.clone();
        let stdout_task = spawn_reader(
            stdout,
            false,
            request.capture_limit,
            request.event_chunk_bytes,
            output_tx.clone(),
            overflow_tx.clone(),
            request.overflow.clone(),
        );
        let stderr_task = spawn_reader(
            stderr,
            true,
            request.capture_limit,
            request.event_chunk_bytes,
            output_tx,
            overflow_tx.clone(),
            request.overflow,
        );
        let deadline = request.timeout;
        // `None` as a termination means the wait has to be settled against
        // the child rather than against the monitor. The `tree.wait()`
        // continuation lives after the `select!` rather than inside one arm
        // because awaiting it in an arm makes the whole future `!Send`, and
        // this `select!` runs inside a spawned task.
        let settled: Option<(
            TerminationReason,
            Option<std::process::ExitStatus>,
            Option<String>,
        )> = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Some((TerminationReason::Cancelled, None, None)),
            _ = sleep(deadline) => Some((TerminationReason::TimedOut, None, None)),
                // Whether the monitor changed or merely closed is not the
                // question; its current value is. A closed monitor reads as
                // `false`, which is the same as a quiet one, and both mean
                // "keep waiting for the child".
                _ = overflow_rx.changed() => {
                    monitor_termination(*overflow_rx.borrow()).map(|t| (t, None, None))
                }
            result = tree.wait() => Some(match result {
                Ok(status) => (TerminationReason::Exited, Some(status), None),
                Err(error) => (TerminationReason::Exited, None, Some(error.to_string())),
            }),
        };
        let (termination, status, wait_error) = match settled {
            Some(settled) => settled,
            None => match tree.wait().await {
                Ok(status) => (TerminationReason::Exited, Some(status), None),
                Err(error) => (TerminationReason::Exited, None, Some(error.to_string())),
            },
        };
        #[cfg(target_os = "linux")]
        let mut resource_unobserved: Vec<eggwork_core::ResourceDimension> = Vec::new();
        #[cfg(target_os = "linux")]
        let mut resource_limits_exceeded = if matches!(&termination, TerminationReason::Exited)
            && let Some(session) = sandbox_session.take()
        {
            let (exceeded, unobserved) = session.resource_status().await;
            resource_unobserved = unobserved;
            exceeded
        } else {
            Vec::new()
        };
        #[cfg(target_os = "linux")]
        if let Some(unit) = resource_unit.as_deref()
            && let Ok(backend) = SystemdCgroupBackend::discover()
            && backend.unit_result(unit).await.as_deref() == Some("oom-kill")
            && !resource_limits_exceeded.contains(&eggwork_core::ResourceDimension::Memory)
        {
            resource_limits_exceeded.push(eggwork_core::ResourceDimension::Memory);
        }
        #[cfg(not(target_os = "linux"))]
        let resource_limits_exceeded: Vec<eggwork_core::ResourceDimension> = Vec::new();
        #[cfg(target_os = "linux")]
        // A dimension the caller requested and the backend installed, but whose
        // post-mortem counter the helper could not open, is not evidence that
        // the limit held. Record `NotApplied` rather than letting it fall
        // through as `Applied`.
        {
            if matches!(setup.resources, ResourceSetupOutcome::Applied { .. }) {
                let requested = &request.resources;
                let unobserved = if matches!(requested.memory_bytes, Requirement::NotRequested) {
                    false
                } else {
                    resource_unobserved.contains(&eggwork_core::ResourceDimension::Memory)
                } || if matches!(requested.pids, Requirement::NotRequested) {
                    false
                } else {
                    resource_unobserved.contains(&eggwork_core::ResourceDimension::Pids)
                };
                if unobserved {
                    let reason = "installed resource limits could not be observed after exit";
                    setup.resources = ResourceSetupOutcome::NotApplied {
                        reason: reason.into(),
                    };
                    // This degradation is a post-mortem fact: the helper
                    // reports the unobservable counter only after the target
                    // exited, so the diagnostic cannot precede the target's
                    // output the way a setup-time degradation does. It still
                    // precedes the terminal event, because the server drains
                    // the output channel before it publishes the terminal
                    // state.
                    for dimension in unobserved_resource_dimensions(requested, &resource_unobserved)
                    {
                        send_diagnostic(&diagnostic_tx, resources_diagnostic(dimension, reason))
                            .await;
                    }
                }
            }
            // The sender is released here, so the output channel keeps closing
            // exactly when the readers finish.
            drop(diagnostic_tx);
        }
        #[cfg(target_os = "linux")]
        let resource_cleanup_warning = if let Some(unit) = resource_unit.as_deref() {
            match SystemdCgroupBackend::discover() {
                Ok(backend) => backend.stop_unit(unit).await.err(),
                Err(_) => Some("systemd scope cleanup was unavailable".into()),
            }
        } else {
            None
        };
        #[cfg(not(target_os = "linux"))]
        let resource_cleanup_warning: Option<String> = None;
        let mut cleanup = CleanupDiagnostics {
            process_group_signal_error: resource_cleanup_warning,
            wait_error,
            stdin_error: None,
        };
        // Tree convergence is the platform's job: on Unix this is
        // SIGTERM to the process group, a grace period, then SIGKILL and a
        // bounded reap of the leader; on Windows it is one
        // `TerminateJobObject` over a Job Object that already owns every
        // descendant. Both return the same three facts, so no shared
        // decision below has to know which backend it is talking to.
        let convergence = if let Some(status) = status {
            // A command can leave background descendants behind and exit
            // while they still hold the output pipes. Reap that process tree
            // before returning, without changing the leader's already-known
            // exit classification.
            let convergence = tree.converge_after_leader_exit().await;
            // Convergence never revokes a status the leader already reported.
            // Dropping it here would turn every successful Windows execution
            // into `Failed`/`Internal` with `exit_code: null`, because the
            // tree owner has nothing left to reap.
            TreeConvergence {
                status: Some(status),
                ..convergence
            }
        } else {
            tree.terminate_and_reap(TERMINATION_GRACE).await
        };
        if let Some(error) = convergence.signal_error {
            cleanup.process_group_signal_error.get_or_insert(error);
        }
        if let Some(error) = convergence.wait_error {
            cleanup.wait_error = Some(error);
        }
        let status = convergence.status;
        if let Some(task) = stdin_task {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => cleanup.stdin_error = Some(error.to_string()),
                Err(error) => cleanup.stdin_error = Some(error.to_string()),
            }
        }
        let (stdout, dropped_out) = join_reader(stdout_task).await?;
        let (stderr, dropped_err) = join_reader(stderr_task).await?;
        Ok(RunnerResult {
            termination,
            exit_code: status.and_then(|s| s.code()),
            stdout,
            stderr,
            cleanup,
            setup,
            resource_request: request.resources,
            resource_limits_exceeded,
            stream_chunks_dropped: dropped_out + dropped_err,
            provenance: request.provenance,
        })
    }
}

/// Whether the node baseline owns `key` on this platform.
///
/// Unix baseline keys come from the shared policy. Windows uses a local,
/// platform-specific baseline. Denied keys are separate: they are dropped,
/// while node-owned keys keep the baseline value.
fn node_owned_key(key: &str, baseline: &[(&'static str, String)]) -> bool {
    baseline
        .iter()
        .any(|(owned, _)| owned.eq_ignore_ascii_case(key))
}

/// Build the child environment from the node baseline and shared policy.
///
/// The baseline wins, denied caller keys are dropped, and other caller keys
/// use last-value-wins handling. Both launch paths use this builder, keeping
/// their environments consistent and free of duplicate keys rejected by the
/// helper. Node provenance variables are appended last.
fn build_child_environment(request: &RunnerRequest) -> Vec<(String, String)> {
    let baseline = baseline_child_environment();
    let mut environment: Vec<(String, String)> = baseline
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect();
    for (key, value) in &request.environment {
        if environment_policy::denied_environment_key(key) {
            continue;
        }
        if node_owned_key(key, &baseline) {
            continue;
        }
        match environment.iter_mut().find(|(existing, _)| existing == key) {
            Some(slot) => slot.1 = value.clone(),
            None => environment.push((key.clone(), value.clone())),
        }
    }
    if let Some(id) = &request.provenance.execution_id {
        environment.push(("EGGWORK_EXECUTION_ID".into(), id.as_str().into()));
    }
    if let Some(generation) = request.provenance.generation {
        environment.push((
            "EGGWORK_EXECUTION_GENERATION".into(),
            generation.get().to_string(),
        ));
    }
    environment
}

/// The keys the node's own baseline installs for an execution child.
///
/// The list is derived from the baseline the runner actually uses, so it
/// cannot drift from it: it is the shared policy's Unix class on Unix, and the
/// runner-local Windows class on Windows.
pub fn baseline_environment_keys() -> Vec<&'static str> {
    baseline_child_environment()
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

fn direct_command(request: &RunnerRequest, cwd: PathBuf) -> Command {
    let mut command = Command::new(&request.argv[0]);
    command
        .args(&request.argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.env_clear();
    for (key, value) in build_child_environment(request) {
        command.env(key, value);
    }
    command
}

/// Return a termination reason only when output overflowed.
///
/// Normal reader completion is not a termination reason; `child.wait()` still
/// supplies the child's exit status. The regression test pins this distinction
/// so the select race cannot turn a normal exit into an internal failure.
fn monitor_termination(overflowed: bool) -> Option<TerminationReason> {
    overflowed.then_some(TerminationReason::OutputLimit)
}

/// Platform-specific baseline entries installed for each child.
///
/// Unix reads the generated shared policy. Windows provides its required
/// system paths and native `PATH` without inheriting the caller's full
/// environment. Caller values for baseline keys are ignored.
fn baseline_child_environment() -> Vec<(&'static str, String)> {
    platform_baseline()
}

/// The Unix baseline: the shared policy's, in the policy's order.
#[cfg(not(windows))]
fn platform_baseline() -> Vec<(&'static str, String)> {
    environment_policy::BASELINE
        .iter()
        .map(|(key, value)| (*key, (*value).to_owned()))
        .collect()
}

/// The Windows baseline: the shared policy's platform-independent part, plus
/// the system directories the loader and `cmd.exe` need.
#[cfg(windows)]
fn platform_baseline() -> Vec<(&'static str, String)> {
    let mut baseline: Vec<(&'static str, String)> = environment_policy::BASELINE_COMMON
        .iter()
        .map(|(key, value)| (*key, (*value).to_owned()))
        .collect();
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    baseline.push(("SystemRoot", system_root.clone()));
    baseline.push(("windir", system_root.clone()));
    baseline.push(("PATH", format!(r"{system_root}\System32;{system_root}")));
    baseline
}

#[cfg(target_os = "linux")]
fn wrap_resource_command(
    command: Command,
    resources: &ResourceSetupRequest,
    outcome: &ResourceSetupOutcome,
) -> Result<(Command, Option<String>), RunnerError> {
    match outcome {
        ResourceSetupOutcome::Applied { backend } if backend == "systemd-cgroup-v2" => {
            let backend = SystemdCgroupBackend::discover().map_err(RunnerError::Setup)?;
            let (command, unit) = backend.wrap(command, resources)?;
            Ok((command, Some(unit)))
        }
        _ => Ok((command, None)),
    }
}

#[cfg(target_os = "linux")]
#[derive(Serialize)]
struct SandboxLaunchSpec {
    schema_version: u16,
    profile: Option<String>,
    root: PathBuf,
    cwd: PathBuf,
    argv: Vec<String>,
    environment: Vec<(String, String)>,
    resource_memory_bytes: Option<u64>,
    resource_cpu_millis: Option<u64>,
    resource_pids: Option<u32>,
}

#[cfg(target_os = "linux")]
struct SandboxChannel {
    _directory: tempfile::TempDir,
    spec_path: PathBuf,
    status_path: PathBuf,
    listener: UnixListener,
    helper_path: PathBuf,
}

#[cfg(target_os = "linux")]
struct SandboxSession {
    stream: tokio::net::UnixStream,
}

#[cfg(target_os = "linux")]
impl SandboxSession {
    /// Bit 2 of the helper's post-mortem status byte: a counter the caller asked
    /// about could not be opened, so the helper cannot say the limit held.
    const STATUS_UNOBSERVED: u8 = 1 << 2;

    /// Returns the dimensions the helper observed being exceeded, and the
    /// dimensions it could not observe at all. The second list must not be
    /// reported as "not exceeded": the limit was real, we just could not watch
    /// it, so the caller records `NotApplied` instead of `Applied`.
    async fn resource_status(
        mut self,
    ) -> (
        Vec<eggwork_core::ResourceDimension>,
        Vec<eggwork_core::ResourceDimension>,
    ) {
        let mut status = [0u8; 1];
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), self.stream.read_exact(&mut status)).await,
            Ok(Ok(_))
        ) {
            return (Vec::new(), Vec::new());
        }
        let mut exceeded = Vec::new();
        let mut unobserved = Vec::new();
        if status[0] & 1 != 0 {
            exceeded.push(eggwork_core::ResourceDimension::Memory);
        }
        if status[0] & 2 != 0 {
            exceeded.push(eggwork_core::ResourceDimension::Pids);
        }
        if status[0] & Self::STATUS_UNOBSERVED != 0 {
            unobserved.extend([
                eggwork_core::ResourceDimension::Memory,
                eggwork_core::ResourceDimension::Pids,
            ]);
        }
        (exceeded, unobserved)
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
enum SandboxWaitError {
    BeforeTarget(RunnerError),
    AfterTarget(RunnerError),
}

#[cfg(target_os = "linux")]
impl SandboxWaitError {
    fn into_runner_error(self) -> RunnerError {
        match self {
            Self::BeforeTarget(error) | Self::AfterTarget(error) => error,
        }
    }
}

/// Decode the helper's status byte.
///
/// `Ok(true)` means resource counters follow; `Ok(false)` means the run
/// continues without them. Refusals happen before the target starts and retain
/// their status code for diagnostics.
#[cfg(target_os = "linux")]
fn classify_helper_status(status: u8) -> Result<bool, SandboxWaitError> {
    match status {
        0 | 2 | 3 | 30 | 31 => Ok(false),
        1 => Ok(true),
        code if code >= 4 => Err(SandboxWaitError::BeforeTarget(RunnerError::Setup(format!(
            "sandbox helper rejected its launch specification (status {code})"
        )))),
        _ => Err(SandboxWaitError::BeforeTarget(RunnerError::Setup(
            "sandbox status was invalid".into(),
        ))),
    }
}

#[cfg(target_os = "linux")]
impl SandboxChannel {
    fn new(
        helper_path: &Path,
        request: &RunnerRequest,
        cwd: PathBuf,
        resources_applied: bool,
    ) -> Result<Self, RunnerError> {
        let profile = match &request.sandbox {
            SandboxRequest::BestEffort { profile } | SandboxRequest::Required { profile }
                if profile == WORKSPACE_RW_PROFILE =>
            {
                Some(profile.clone())
            }
            SandboxRequest::None if request.resources.is_requested() => None,
            _ => return Err(RunnerError::Setup("unsupported sandbox profile".into())),
        };
        let directory = tempfile::Builder::new()
            .prefix("eggwork-sandbox-")
            .tempdir()
            .map_err(|_| RunnerError::Setup("sandbox channel unavailable".into()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(|_| RunnerError::Setup("sandbox channel unavailable".into()))?;
        }
        let spec_path = directory.path().join("spec.json");
        let status_path = directory.path().join("status.sock");
        let listener = UnixListener::bind(&status_path)
            .map_err(|_| RunnerError::Setup("sandbox status channel unavailable".into()))?;
        // The launch specification's environment is built by the same function
        // that builds the direct child's environment, so the two paths cannot
        // disagree about a node-owned key's value. Before that was true, the
        // direct path applied the caller's `LANG` *after* the node's baseline
        // while the specification carried both, and the helper refused the
        // resulting duplicate-key specification.
        let environment = build_child_environment(request);
        let spec = SandboxLaunchSpec {
            schema_version: 1,
            profile,
            root: request
                .root
                .canonicalize()
                .map_err(|_| RunnerError::InvalidRoot("execution root is invalid".into()))?,
            cwd,
            argv: request.argv.clone(),
            environment,
            resource_memory_bytes: resources_applied
                .then_some(match request.resources.memory_bytes {
                    Requirement::BestEffort(value) | Requirement::Required(value) => Some(value),
                    Requirement::NotRequested => None,
                })
                .flatten(),
            resource_cpu_millis: resources_applied
                .then_some(match request.resources.cpu_millis {
                    Requirement::BestEffort(value) | Requirement::Required(value) => Some(value),
                    Requirement::NotRequested => None,
                })
                .flatten(),
            resource_pids: resources_applied
                .then_some(match request.resources.pids {
                    Requirement::BestEffort(value) | Requirement::Required(value) => Some(value),
                    Requirement::NotRequested => None,
                })
                .flatten(),
        };
        let encoded = serde_json::to_vec(&spec)
            .map_err(|_| RunnerError::Setup("sandbox specification is invalid".into()))?;
        if encoded.len() > 1024 * 1024 {
            return Err(RunnerError::Setup(
                "sandbox specification exceeds its bound".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&spec_path)
                .map_err(|_| RunnerError::Setup("sandbox specification unavailable".into()))?;
            file.write_all(&encoded)
                .map_err(|_| RunnerError::Setup("sandbox specification unavailable".into()))?;
        }
        Ok(Self {
            _directory: directory,
            spec_path,
            status_path,
            listener,
            helper_path: helper_path.to_path_buf(),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.helper_path);
        command
            .arg("--spec")
            .arg(&self.spec_path)
            .arg("--status")
            .arg(&self.status_path)
            .current_dir(self.spec_path.parent().expect("spec parent"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear();
        // The helper's own environment is the node's baseline, from the same
        // shared policy the target's environment is built from. `env_clear()`
        // matters here: the helper must not inherit the daemon's `LD_*`,
        // `GIT_*`, or cloud variables while it is parsing a specification that
        // a deny-list was supposed to keep clean.
        for (key, value) in environment_policy::BASELINE {
            command.env(key, value);
        }
        command
    }

    async fn wait(
        self,
        _child: &mut tokio::process::Child,
        cancellation: CancellationToken,
    ) -> Result<Option<SandboxSession>, SandboxWaitError> {
        let (mut stream, _) = tokio::select! {
            _ = cancellation.cancelled() => return Err(SandboxWaitError::BeforeTarget(RunnerError::CancelledBeforeSpawn)),
            accepted = tokio::time::timeout(Duration::from_secs(5), self.listener.accept()) => {
                accepted
                    .map_err(|_| SandboxWaitError::BeforeTarget(RunnerError::Setup("sandbox status timed out".into())))?
                    .map_err(|_| SandboxWaitError::BeforeTarget(RunnerError::Setup("sandbox status channel failed".into())))?
            }
        };
        let mut status = [0u8; 1];
        tokio::select! {
            _ = cancellation.cancelled() => Err(SandboxWaitError::BeforeTarget(RunnerError::CancelledBeforeSpawn)),
            result = tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut status)) => {
                result
                    .map_err(|_| SandboxWaitError::BeforeTarget(RunnerError::Setup("sandbox status timed out".into())))?
                    .map_err(|_| SandboxWaitError::BeforeTarget(RunnerError::Setup("sandbox status channel failed".into())))?;
                match classify_helper_status(status[0]) {
                    Ok(false) => Ok(None),
                    Ok(true) => {
                        let mut target_spawned = [0u8; 1];
                        tokio::select! {
                            _ = cancellation.cancelled() => return Err(SandboxWaitError::AfterTarget(RunnerError::CancelledBeforeSpawn)),
                            result = tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut target_spawned)) => {
                                result
                                    .map_err(|_| SandboxWaitError::AfterTarget(RunnerError::Setup("sandbox status timed out".into())))?
                                    .map_err(|_| SandboxWaitError::AfterTarget(RunnerError::Setup("sandbox status channel failed".into())))?;
                            }
                        }
                        match target_spawned[0] {
                            1 => Ok(Some(SandboxSession { stream })),
                            0 => Err(SandboxWaitError::AfterTarget(RunnerError::Spawn("sandbox target could not start".into()))),
                            _ => Err(SandboxWaitError::AfterTarget(RunnerError::Setup("sandbox status was invalid".into()))),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }
}

fn spawn_reader<R: AsyncRead + Unpin + Send + 'static>(
    mut reader: R,
    stderr: bool,
    capture_limit: usize,
    chunk_limit: usize,
    output_tx: mpsc::Sender<OutputChunk>,
    overflow_tx: watch::Sender<bool>,
    overflow: CoreOverflowPolicy,
) -> JoinHandle<Result<(BoundedCapture, u64), RunnerError>> {
    tokio::spawn(async move {
        let mut capture = BoundedCapture::default();
        let mut dropped = 0;
        let mut buffer = vec![0; READ_BUFFER.min(chunk_limit)];
        loop {
            let count = reader
                .read(&mut buffer)
                .await
                .map_err(|e| RunnerError::Io(e.to_string()))?;
            if count == 0 {
                break;
            }
            let before = capture.total_bytes;
            capture.push(&buffer[..count], capture_limit);
            if matches!(overflow, CoreOverflowPolicy::Terminate)
                && before <= capture_limit as u64
                && capture.total_bytes > capture_limit as u64
            {
                let _ = overflow_tx.send(true);
            }
            for bytes in buffer[..count].chunks(chunk_limit) {
                let chunk = if stderr {
                    OutputChunk::Stderr(bytes.to_vec())
                } else {
                    OutputChunk::Stdout(bytes.to_vec())
                };
                if output_tx.try_send(chunk).is_err() {
                    dropped += bytes.len() as u64;
                }
            }
        }
        Ok((capture, dropped))
    })
}

async fn join_reader(
    task: JoinHandle<Result<(BoundedCapture, u64), RunnerError>>,
) -> Result<(BoundedCapture, u64), RunnerError> {
    task.await.map_err(|e| RunnerError::Io(e.to_string()))?
}

impl From<RunnerResult> for ExecutionResult {
    fn from(result: RunnerResult) -> Self {
        result.execution_result()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::Arc;

    fn request(root: &Path, args: &[&str]) -> RunnerRequest {
        RunnerRequest {
            argv: args.iter().map(|v| (*v).to_owned()).collect(),
            root: root.into(),
            cwd: None,
            environment: vec![],
            stdin: StdinPolicy::Null,
            timeout: Duration::from_secs(4),
            capture_limit: 1024,
            event_chunk_bytes: 128,
            overflow: CoreOverflowPolicy::Truncate,
            provenance: ExecutionProvenance::default(),
            sandbox: SandboxRequest::None,
            resources: ResourceSetupRequest {
                memory_bytes: Requirement::NotRequested,
                cpu_millis: Requirement::NotRequested,
                pids: Requirement::NotRequested,
            },
        }
    }

    #[test]
    fn a_settled_output_monitor_is_not_a_termination_reason() {
        // Overflowing the capture limit ends the execution; the monitor merely
        // finishing does not. Reporting the second case as an exit discarded the
        // child's real status.
        assert_eq!(
            monitor_termination(true),
            Some(TerminationReason::OutputLimit)
        );
        assert_eq!(monitor_termination(false), None);
    }

    /// Ensure normal pipe completion does not discard the child's exit status.
    /// Repeated short-lived runs exercise the race between reader EOF and
    /// `child.wait()` on every supported platform.
    #[tokio::test]
    async fn a_short_lived_child_that_closes_its_pipes_keeps_its_exit_status() {
        let temp = tempfile::tempdir().unwrap();
        let (argv, expected): (Vec<String>, &[u8]) = if cfg!(windows) {
            let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned());
            (
                vec![
                    shell,
                    "/C".to_owned(),
                    "echo qualification-marker".to_owned(),
                ],
                b"qualification-marker\r\n",
            )
        } else {
            (
                vec!["/bin/echo".to_owned(), "qualification-marker".to_owned()],
                b"qualification-marker\n",
            )
        };
        for _ in 0..32 {
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            let mut req = request(temp.path(), &args);
            req.timeout = Duration::from_secs(10);
            let result = run(req).await.unwrap();
            // This assertion does not by itself reproduce the race: on Linux the
            // child wait usually wins it, and the loop passes either way. The
            // deterministic guard is `a_settled_output_monitor_is_not_a_
            // termination_reason`; this is the end-to-end invariant.
            assert_eq!(
                result.exit_code,
                Some(0),
                "a closed output monitor must not be reported as a missing exit status: {result:?}"
            );
            assert_eq!(result.termination, TerminationReason::Exited);
            assert_eq!(result.stdout.head, expected);
            assert_eq!(result.cleanup.wait_error, None);
        }
    }

    #[tokio::test]
    async fn unsupported_resource_limits_are_reported_or_rejected_before_spawn() {
        let temp = tempfile::tempdir().unwrap();
        let runner = LocalProcessRunner::new(NoExecutionSetup);
        let marker = temp.path().join("required-limit-target-ran");
        // The required case must be refused *before* spawn, so the target is a
        // command whose only job is to leave the marker behind. If the refusal
        // ever moved after spawn, this file would exist.
        let (succeeds, leaves_marker): (Vec<String>, Vec<String>) = if cfg!(windows) {
            (
                vec![
                    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned()),
                    "/C".to_owned(),
                    "exit 0".to_owned(),
                ],
                vec![
                    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned()),
                    "/C".to_owned(),
                    format!("type nul > {}", marker.display()),
                ],
            )
        } else {
            (
                // `/usr/bin/true`, not `/bin/true`: macOS keeps `true` in
                // `/usr/bin`, and `/bin` holds only the small set of tools
                // that were always there. A Unix-shaped absolute path is a
                // macOS-shaped failure.
                vec!["/usr/bin/true".to_owned()],
                vec![
                    "/usr/bin/touch".to_owned(),
                    marker.to_str().unwrap().to_owned(),
                ],
            )
        };

        let succeeds: Vec<&str> = succeeds.iter().map(String::as_str).collect();
        let leaves_marker: Vec<&str> = leaves_marker.iter().map(String::as_str).collect();
        let mut best_effort = request(temp.path(), &succeeds);
        best_effort.resources.memory_bytes = Requirement::BestEffort(1024 * 1024);
        let (tx, _rx) = mpsc::channel(4);
        let result = runner
            .run(best_effort, CancellationToken::new(), tx)
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(matches!(
            result.execution_result().resources.unwrap().memory_bytes,
            eggwork_core::ResourceDimensionResult::NotApplied { .. }
        ));

        let mut required = request(temp.path(), &leaves_marker);
        required.resources.pids = Requirement::Required(4);
        let (tx, _rx) = mpsc::channel(4);
        assert!(matches!(
            runner.run(required, CancellationToken::new(), tx).await,
            Err(RunnerError::RequiredSetupUnavailable)
        ));
        assert!(!marker.exists());
    }

    #[test]
    fn runner_request_debug_redacts_command_and_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = request(temp.path(), &["tool", "--token=argv-secret"]);
        request
            .environment
            .push(("API_TOKEN".into(), "env-secret".into()));
        request.stdin = StdinPolicy::Bytes(b"stdin-secret".to_vec());
        let debug = format!("{request:?}");
        for secret in ["argv-secret", "env-secret", "stdin-secret"] {
            assert!(!debug.contains(secret), "Debug leaked {secret}");
        }
    }
    async fn run(req: RunnerRequest) -> Result<RunnerResult, RunnerError> {
        let runner = LocalProcessRunner::default();
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(2);
        runner.run(req, token, tx).await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn captures_exit_stdin_and_sanitizes_environment() {
        let temp = tempfile::tempdir().unwrap();
        let mut req = request(
            temp.path(),
            &[
                "sh",
                "-c",
                "read x; printf '%s:%s' \"$x\" \"${LD_PRELOAD-unset}\"; printf err >&2; exit 7",
            ],
        );
        req.stdin = StdinPolicy::Bytes(b"input\n".to_vec());
        req.environment.push(("LD_PRELOAD".into(), "bad".into()));
        let result = run(req).await.unwrap();
        assert_eq!(result.exit_code, Some(7));
        assert_eq!(result.termination, TerminationReason::Exited);
        assert_eq!(result.cleanup.wait_error, None);
        assert_eq!(result.stdout.head, b"input:unset");
        assert_eq!(result.stderr.head, b"err");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn completed_output_readers_do_not_report_monitor_closed_as_cleanup_failure() {
        let temp = tempfile::tempdir().unwrap();
        for _ in 0..16 {
            let req = request(temp.path(), &["/bin/echo", "short-lived-output"]);
            let result = run(req).await.unwrap();
            assert_eq!(result.exit_code, Some(0));
            assert_eq!(result.stdout.head, b"short-lived-output\n");
            assert_eq!(result.cleanup.wait_error, None);
        }

        let marker = temp.path().join("target-finished-after-closing-output");
        let req = request(
            temp.path(),
            &[
                "/bin/sh",
                "-c",
                "exec >/dev/null 2>&1; sleep 0.1; printf completed > \"$1\"",
                "sh",
                marker.to_str().unwrap(),
            ],
        );
        let result = run(req).await.unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.cleanup.wait_error, None);
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "completed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounds_capture_and_streaming_under_slow_receiver() {
        let temp = tempfile::tempdir().unwrap();
        let mut req = request(temp.path(), &["sh", "-c", "head -c 200000 /dev/zero"]);
        req.capture_limit = 128;
        req.event_chunk_bytes = 64;
        let runner = LocalProcessRunner::default();
        let (tx, rx) = mpsc::channel(1);
        let result = runner.run(req, CancellationToken::new(), tx).await.unwrap();
        assert!(result.stdout.total_bytes >= 200000);
        assert_eq!(result.stdout.head.len() + result.stdout.tail.len(), 128);
        assert!(result.stream_chunks_dropped > 0);
        drop(rx);
    }

    #[cfg(unix)]
    #[ignore = "manual output/drain characterization; no timing threshold"]
    #[tokio::test]
    async fn characterize_output_drain_overhead() {
        let temp = tempfile::tempdir().unwrap();
        for bytes in [4_096usize, 8 * 1024 * 1024] {
            let command = format!("head -c {bytes} /dev/zero");
            let mut req = request(temp.path(), &["sh", "-c", &command]);
            req.capture_limit = 64 * 1024;
            req.event_chunk_bytes = 16 * 1024;
            let runner = LocalProcessRunner::default();
            let (tx, _rx) = mpsc::channel(16);
            let started = std::time::Instant::now();
            let result = runner.run(req, CancellationToken::new(), tx).await.unwrap();
            let elapsed = started.elapsed();
            let captured = result.stdout.head.len() + result.stdout.tail.len();
            assert_eq!(result.stdout.total_bytes, bytes as u64);
            assert!(captured <= 64 * 1024);
            eprintln!(
                "runner output characterization: bytes={} elapsed_ms={} throughput_mib_s={:.2} retained_bytes={}",
                result.stdout.total_bytes,
                elapsed.as_millis(),
                bytes as f64 / elapsed.as_secs_f64() / (1024.0 * 1024.0),
                captured,
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_and_cancellation_kill_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let mut req = request(temp.path(), &["sh", "-c", "sleep 30 & wait"]);
        req.timeout = Duration::from_millis(100);
        let result = run(req).await.unwrap();
        assert_eq!(result.termination, TerminationReason::TimedOut);
        let runner = Arc::new(LocalProcessRunner::default());
        let token = CancellationToken::new();
        let child_token = token.clone();
        let root = temp.path().to_owned();
        let task = tokio::spawn(async move {
            let (tx, _rx) = mpsc::channel(2);
            runner
                .run(
                    request(&root, &["sh", "-c", "sleep 30 & wait"]),
                    child_token,
                    tx,
                )
                .await
        });
        sleep(Duration::from_millis(100)).await;
        token.cancel();
        assert_eq!(
            task.await.unwrap().unwrap().termination,
            TerminationReason::Cancelled
        );
    }

    #[tokio::test]
    async fn cancellation_before_spawn_and_spawn_failure_are_typed() {
        let temp = tempfile::tempdir().unwrap();
        let runner = LocalProcessRunner::default();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let (tx, _rx) = mpsc::channel(1);
        assert!(matches!(
            runner
                .run(request(temp.path(), &["true"]), cancelled, tx)
                .await,
            Err(RunnerError::CancelledBeforeSpawn)
        ));
        // A program that cannot be created must fail as a typed spawn error on
        // every platform, and it must fail before a tree owner exists: there is
        // no job, no process group, and nothing to clean up. A missing
        // executable is the cheapest way to prove that the platform's
        // create-then-own sequence has no partial state to leak.
        let missing = if cfg!(windows) {
            r"definitely-missing-eggwork-command.exe"
        } else {
            "/definitely/missing/eggwork-command"
        };
        assert!(matches!(
            run(request(temp.path(), &[missing])).await,
            Err(RunnerError::Spawn(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cwd_is_confined_and_output_limit_terminates() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("work")).unwrap();
        let mut cwd = request(temp.path(), &["pwd"]);
        cwd.cwd = Some(RelativePath::new("work").unwrap());
        let result = run(cwd).await.unwrap();
        // The child reports the *physical* working directory: macOS resolves
        // `/var` to `/private/var`, so the expectation is canonicalized
        // rather than compared against the path the temporary directory was
        // created under.
        let expected = temp
            .path()
            .join("work")
            .canonicalize()
            .expect("the confined working directory is canonical");
        assert_eq!(
            String::from_utf8_lossy(&result.stdout.head).trim(),
            expected.to_string_lossy()
        );

        let mut overflow = request(
            temp.path(),
            &["sh", "-c", "while :; do printf 0123456789; done"],
        );
        overflow.capture_limit = 64;
        overflow.overflow = CoreOverflowPolicy::Terminate;
        overflow.timeout = Duration::from_secs(3);
        assert_eq!(
            run(overflow).await.unwrap().termination,
            TerminationReason::OutputLimit
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_kills_descendants_that_ignore_sigterm() {
        let temp = tempfile::tempdir().unwrap();
        let mut req = request(
            temp.path(),
            &["sh", "-c", "trap '' TERM; sleep 30 & echo $!; wait"],
        );
        req.timeout = Duration::from_millis(100);
        let result = run(req).await.unwrap();
        assert_eq!(result.termination, TerminationReason::TimedOut);
        let pid = String::from_utf8_lossy(&result.stdout.head)
            .trim()
            .parse::<i32>()
            .unwrap();
        assert!(descendant_converged(pid).await);
    }

    /// Wait until `/proc` shows a descendant gone or zombie. Group `SIGKILL`
    /// delivery is asynchronous after leader reap, so poll instead of assuming
    /// an immediate terminal state.
    #[cfg(target_os = "linux")]
    async fn descendant_converged(pid: i32) -> bool {
        for _ in 0..200 {
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Err(_) => return true,
                Ok(stat) => {
                    if stat.split_whitespace().nth(2) == Some("Z") {
                        return true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn natural_exit_reaps_background_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let result = run(request(temp.path(), &["sh", "-c", "sleep 30 & echo $!"]))
            .await
            .unwrap();
        assert_eq!(result.termination, TerminationReason::Exited);
        let pid = String::from_utf8_lossy(&result.stdout.head)
            .trim()
            .parse::<i32>()
            .unwrap();
        let proc_state = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        assert!(proc_state.is_err() || proc_state.unwrap().split_whitespace().nth(2) == Some("Z"));
    }

    #[test]
    fn output_capture_retains_head_tail_with_totals() {
        let mut c = BoundedCapture::default();
        c.push(b"abcdefghij", 6);
        assert_eq!(c.head, b"abc");
        assert_eq!(c.tail.iter().copied().collect::<Vec<_>>(), b"hij");
        assert_eq!(c.omitted_bytes, 4);
    }

    #[test]
    fn cleanup_warning_does_not_rewrite_process_outcome() {
        let result = RunnerResult {
            termination: TerminationReason::Exited,
            exit_code: Some(9),
            stdout: BoundedCapture::default(),
            stderr: BoundedCapture::default(),
            cleanup: CleanupDiagnostics {
                process_group_signal_error: Some("permission denied".into()),
                wait_error: None,
                stdin_error: None,
            },
            setup: SetupOutcome {
                sandbox: SandboxOutcome::NotRequested,
                resources: ResourceSetupOutcome::NotRequested,
            },
            resource_request: ResourceSetupRequest {
                memory_bytes: Requirement::NotRequested,
                cpu_millis: Requirement::NotRequested,
                pids: Requirement::NotRequested,
            },
            resource_limits_exceeded: Vec::new(),
            stream_chunks_dropped: 0,
            provenance: ExecutionProvenance::default(),
        };
        let terminal = result.execution_result();
        assert_eq!(terminal.state, ExecutionState::Failed);
        assert_eq!(terminal.exit_code, Some(9));
        assert_eq!(
            terminal.cleanup_warning.as_deref(),
            Some("permission denied")
        );
    }

    #[test]
    fn capability_feature_string_is_frozen_for_workspace_rw_profile() {
        assert_eq!(
            LANDLOCK_WORKSPACE_RW_CAPABILITY,
            "isolation.landlock.workspace-rw.v1"
        );
    }

    /// The three cgroup feature names are frozen byte-for-byte.
    ///
    /// A node advertises these and a server admits a required resource
    /// dimension only when the advertised name matches, so a renamed byte
    /// turns every memory-, CPU- or pids-limited request into a capability
    /// refusal. The values are asserted here rather than only through the
    /// constants, so a value change is a test failure even when the constant
    /// and its consumer are edited together.
    #[test]
    fn cgroup_capability_feature_strings_are_frozen() {
        assert_eq!(CGROUPS_V2_MEMORY_CAPABILITY, "resources.cgroups-v2.memory");
        assert_eq!(CGROUPS_V2_CPU_CAPABILITY, "resources.cgroups-v2.cpu");
        assert_eq!(CGROUPS_V2_PIDS_CAPABILITY, "resources.cgroups-v2.pids");
    }

    /// The sandbox profile name is frozen byte-for-byte.
    ///
    /// The helper is a standalone binary with its own literal, so the two
    /// spellings are pinned together by the helper's test rather than by a
    /// shared constant. This assertion is the runner's half of that pair.
    #[test]
    fn the_workspace_rw_profile_name_is_frozen() {
        assert_eq!(WORKSPACE_RW_PROFILE, "workspace_rw");
    }

    /// The advertisement site cannot invent a feature name.
    ///
    /// The probe table is the source of every resource feature the runner can
    /// advertise, and it names them with the exported constants. A literal
    /// typed back into the table would fail this on any host, because the
    /// check reads the table rather than a list of names written here.
    #[test]
    fn the_probe_table_advertises_exactly_the_exported_constants() {
        let names: Vec<&str> = CGROUP_PROBE_TABLE
            .iter()
            .map(|(feature, _)| *feature)
            .collect();
        assert_eq!(
            names,
            [
                CGROUPS_V2_MEMORY_CAPABILITY,
                CGROUPS_V2_CPU_CAPABILITY,
                CGROUPS_V2_PIDS_CAPABILITY,
            ]
        );
    }

    #[tokio::test]
    async fn no_setup_advertises_empty_dynamic_capabilities() {
        let setup = NoExecutionSetup;
        let runtime = setup.execution_capabilities().await;
        assert!(runtime.is_empty());
    }

    /// The reserved-key set matches the baseline keys; drift would make the
    /// helper reject a launch the runner accepted.
    #[test]
    fn the_node_owned_class_is_the_node_owned_baseline() {
        let baseline_keys: Vec<&str> = environment_policy::BASELINE
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(
            environment_policy::RESERVED_KEYS,
            baseline_keys.as_slice(),
            "the node-owned class and the node-owned baseline must be the same set"
        );
        for (key, value) in environment_policy::BASELINE {
            assert_eq!(
                environment_policy::baseline_value(key),
                Some(*value),
                "the baseline value of {key} must be the policy's value"
            );
            assert!(
                environment_policy::is_reserved_key(key),
                "{key} is in the baseline and must be node-owned"
            );
        }
    }

    /// Caller values cannot override node baseline keys.
    ///
    /// The baseline is platform-specific, so the test checks every installed
    /// key appears once with the node's value.
    #[test]
    fn a_caller_supplied_node_owned_key_does_not_replace_the_node_value() {
        let root = tempfile::tempdir().expect("temp dir");
        let mut req = request(root.path(), &["/bin/sh", "-c", "true"]);
        req.environment = vec![
            ("LANG".into(), "en_US.UTF-8".into()),
            ("LC_ALL".into(), "en_US.UTF-8".into()),
            ("CI".into(), "false".into()),
            ("lang".into(), "de_DE.UTF-8".into()),
        ];
        let environment = build_child_environment(&req);
        let node_owned = baseline_child_environment();
        for (key, node_value) in &node_owned {
            let matching: Vec<&String> = environment
                .iter()
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value)
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "{key} must appear exactly once in the child environment"
            );
            assert_eq!(matching[0], node_value, "{key} must carry the node's value");
            // A differently-cased spelling of a node-owned key is the same key,
            // so the caller's spelling is discarded too.
            let lower = key.to_ascii_lowercase();
            if lower != *key {
                assert!(
                    !environment.iter().any(|(name, _)| name == &lower),
                    "{lower} is a differently-cased spelling of the node-owned {key}"
                );
            }
        }
    }

    /// A denied key is dropped, and only dropped.
    ///
    /// The runner is the request-normalisation boundary: it drops the key
    /// silently and says nothing, because a hardened environment has to be a
    /// property of the node rather than something the caller has to get right.
    /// The helper's refusal is a different layer and a different behaviour.
    #[test]
    fn a_caller_supplied_denied_key_does_not_reach_the_child() {
        let root = tempfile::tempdir().expect("temp dir");
        let mut req = request(root.path(), &["/bin/sh", "-c", "true"]);
        req.environment = vec![
            ("LD_PRELOAD".into(), "/tmp/evil.so".into()),
            ("AWS_SECRET_ACCESS_KEY".into(), "secret".into()),
            ("PYTHONPATH".into(), "/tmp".into()),
            ("KEEP_ME".into(), "kept".into()),
        ];
        let environment = build_child_environment(&req);
        for denied in ["LD_PRELOAD", "AWS_SECRET_ACCESS_KEY", "PYTHONPATH"] {
            assert!(
                !environment.iter().any(|(name, _)| name == denied),
                "{denied} must not reach the child"
            );
        }
        assert!(
            environment
                .iter()
                .any(|(name, value)| name == "KEEP_ME" && value == "kept"),
            "a caller-controlled key must reach the child"
        );
    }

    /// A repeated caller key is resolved to its last value.
    ///
    /// The launch specification must not carry a duplicate key: the helper
    /// refuses a specification that does, and a best-effort request answers
    /// that refusal by re-spawning the target with no Landlock ruleset.
    #[test]
    fn a_repeated_caller_key_is_resolved_to_one_entry() {
        let root = tempfile::tempdir().expect("temp dir");
        let mut req = request(root.path(), &["/bin/sh", "-c", "true"]);
        req.environment = vec![
            ("REPEATED".into(), "first".into()),
            ("REPEATED".into(), "second".into()),
        ];
        let environment = build_child_environment(&req);
        let matching: Vec<&String> = environment
            .iter()
            .filter(|(name, _)| name == "REPEATED")
            .map(|(_, value)| value)
            .collect();
        assert_eq!(matching, vec!["second"]);
    }

    /// The launch specification carries each node-owned key once with the
    /// node's value. Duplicates are rejected by the helper.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_launch_specification_carries_one_entry_per_node_owned_key() {
        let root = tempfile::tempdir().expect("temp dir");
        let mut req = request(root.path(), &["/bin/sh", "-c", "true"]);
        req.sandbox = SandboxRequest::BestEffort {
            profile: "workspace_rw".into(),
        };
        req.environment = vec![("LANG".into(), "en_US.UTF-8".into())];
        let channel = SandboxChannel::new(
            Path::new("/nonexistent/eggwork-sandbox-helper"),
            &req,
            root.path().to_path_buf(),
            false,
        )
        .expect("sandbox channel");
        let spec: serde_json::Value =
            serde_json::from_slice(&std::fs::read(channel.spec_path).expect("spec"))
                .expect("spec json");
        let environment = spec["environment"]
            .as_array()
            .expect("environment array")
            .iter()
            .map(|entry| {
                (
                    entry[0].as_str().expect("key").to_owned(),
                    entry[1].as_str().expect("value").to_owned(),
                )
            })
            .collect::<Vec<(String, String)>>();
        for key in environment_policy::RESERVED_KEYS {
            let matching: Vec<&String> = environment
                .iter()
                .filter(|(name, _)| name == *key)
                .map(|(_, value)| value)
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "the specification must carry exactly one {key}"
            );
            assert_eq!(
                matching[0],
                environment_policy::baseline_value(key).expect("baseline value"),
                "the specification's {key} must carry the node's value"
            );
        }
        assert_eq!(
            environment_policy::PATH_VALUE,
            "/usr/bin:/bin",
            "the `PATH` contract is a Linux/Unix contract"
        );
        assert!(
            environment
                .iter()
                .any(|(name, value)| name == "PATH" && value == environment_policy::PATH_VALUE),
            "the specification's PATH must carry the contract value"
        );
    }

    /// The direct and helper launch paths build the same child environment.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_direct_path_and_the_launch_specification_agree() {
        let root = tempfile::tempdir().expect("temp dir");
        let mut req = request(root.path(), &["/bin/sh", "-c", "true"]);
        req.sandbox = SandboxRequest::BestEffort {
            profile: "workspace_rw".into(),
        };
        req.environment = vec![
            ("LANG".into(), "en_US.UTF-8".into()),
            ("CI".into(), "false".into()),
            ("CALLER_KEY".into(), "caller".into()),
        ];
        let direct = build_child_environment(&req);
        let channel = SandboxChannel::new(
            Path::new("/nonexistent/eggwork-sandbox-helper"),
            &req,
            root.path().to_path_buf(),
            false,
        )
        .expect("sandbox channel");
        let spec: serde_json::Value =
            serde_json::from_slice(&std::fs::read(channel.spec_path).expect("spec"))
                .expect("spec json");
        let specified = spec["environment"]
            .as_array()
            .expect("environment array")
            .iter()
            .map(|entry| {
                (
                    entry[0].as_str().expect("key").to_owned(),
                    entry[1].as_str().expect("value").to_owned(),
                )
            })
            .collect::<Vec<(String, String)>>();
        assert_eq!(direct, specified);
    }

    /// The environment-policy refusal is its own status, and the runner
    /// treats it as a failure that happened before the target existed.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_helper_status_bytes_classify_a_refusal_before_the_target() {
        for (byte, expected) in [
            (0u8, false),
            (2, false),
            (3, false),
            (30, false),
            (31, false),
        ] {
            assert!(
                matches!(classify_helper_status(byte), Ok(value) if value == expected),
                "status {byte} must not be a refusal"
            );
        }
        assert!(matches!(classify_helper_status(1), Ok(true)));
        for code in [12u8, 13] {
            match classify_helper_status(code) {
                Err(SandboxWaitError::BeforeTarget(RunnerError::Setup(message))) => assert!(
                    message.contains(&format!("status {code}")),
                    "the refusal must name its status byte: {message}"
                ),
                _ => panic!("status {code} must be a pre-target refusal"),
            }
        }
    }
}
