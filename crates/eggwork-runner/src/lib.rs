#![forbid(unsafe_code)]

//! Canonical owner of finite, noninteractive process execution.
//!
//! Embedding API: construct a validated [`RunnerRequest`] from an
//! [`eggwork_core::ExecutionSpec`] with [`RunnerRequest::from_spec`], then run
//! it through [`LocalProcessRunner`]. Request fields remain private so callers
//! cannot bypass protocol bounds. Platform setup is supplied through
//! [`ExecutionSetup`]; neither this API nor its result types carry scheduler
//! placement, priority, or project fairness policy.

use eggwork_core::{
    CommandSpec, ExecutionFailure, ExecutionGeneration, ExecutionId, ExecutionResult,
    ExecutionSpec, ExecutionState, MAX_CAPTURE_BYTES, MAX_ENV_COUNT, MAX_ENV_NAME_BYTES,
    MAX_ENV_VALUE_BYTES, MAX_EVENT_CHUNK_BYTES, OverflowPolicy as CoreOverflowPolicy, RelativePath,
    Requirement, StdinPolicy as CoreStdinPolicy,
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    io::Write,
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
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;

const READ_BUFFER: usize = 8192;
const TERMINATION_GRACE: Duration = Duration::from_millis(300);

/// Neutral attribution carried into the child environment and result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecutionProvenance {
    pub execution_id: Option<ExecutionId>,
    pub generation: Option<ExecutionGeneration>,
}

/// A bounded output chunk. Consumers must treat bytes as arbitrary binary data.
#[derive(Clone, PartialEq, Eq)]
pub struct OutputChunk {
    pub stderr: bool,
    pub bytes: Vec<u8>,
}
impl std::fmt::Debug for OutputChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputChunk")
            .field("stderr", &self.stderr)
            .field("length", &self.bytes.len())
            .field("bytes", &"[REDACTED]")
            .finish()
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
        for &byte in bytes {
            if self.head.len() < head_limit {
                self.head.push(byte);
            } else if tail_limit > 0 {
                if self.tail.len() == tail_limit {
                    self.tail.pop_front();
                }
                self.tail.push_back(byte);
            }
        }
        self.omitted_bytes = self
            .total_bytes
            .saturating_sub((self.head.len() + self.tail.len()) as u64);
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
        match timeout(Duration::from_secs(5), child.wait()).await {
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
        let output = timeout(Duration::from_secs(1), command.output())
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
        match timeout(Duration::from_secs(2), child.wait()).await {
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

/// Hook implementations may prepare a command before it is spawned. This
/// milestone ships only an explicit unavailable implementation.
#[async_trait::async_trait]
pub trait ExecutionSetup: Send + Sync {
    async fn prepare(
        &self,
        sandbox: &SandboxRequest,
        resources: &ResourceSetupRequest,
        root: &Path,
    ) -> Result<SetupOutcome, RunnerError>;

    async fn resource_capabilities(&self) -> Vec<String> {
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
                if profile != "workspace_rw" {
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

    async fn resource_capabilities(&self) -> Vec<String> {
        if verify_trusted_helper(&self.helper_path).is_err() {
            return Vec::new();
        }
        let probes = [
            (
                "resources.cgroups-v2.memory",
                ResourceSetupRequest {
                    memory_bytes: Requirement::BestEffort(32 * 1024 * 1024),
                    cpu_millis: Requirement::NotRequested,
                    pids: Requirement::NotRequested,
                },
            ),
            (
                "resources.cgroups-v2.cpu",
                ResourceSetupRequest {
                    memory_bytes: Requirement::NotRequested,
                    cpu_millis: Requirement::BestEffort(100),
                    pids: Requirement::NotRequested,
                },
            ),
            (
                "resources.cgroups-v2.pids",
                ResourceSetupRequest {
                    memory_bytes: Requirement::NotRequested,
                    cpu_millis: Requirement::NotRequested,
                    pids: Requirement::BestEffort(16),
                },
            ),
        ];
        let mut capabilities = Vec::new();
        for (feature, request) in probes {
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

#[cfg(target_os = "linux")]
fn verify_trusted_helper(path: &Path) -> Result<(), String> {
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
fn verify_trusted_helper(_path: &Path) -> Result<(), String> {
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
    /// Create an embedding request with conservative defaults. [`run`](LocalProcessRunner::run)
    /// validates every field before setup or child creation.
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
        spec.validate()
            .map_err(|e| RunnerError::InvalidRequest(e.to_string()))?;
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
                profile: "workspace_rw".into(),
            },
            eggwork_core::IsolationRequirement::Required => SandboxRequest::Required {
                profile: "workspace_rw".into(),
            },
        };
        Ok(Self {
            argv,
            root: root.into(),
            cwd,
            environment: environment.into_iter().map(|e| (e.name, e.value)).collect(),
            stdin: match stdin {
                CoreStdinPolicy::Null => StdinPolicy::Null,
                CoreStdinPolicy::Bytes(bytes) => StdinPolicy::Bytes(bytes),
            },
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

    fn validate(&self) -> Result<PathBuf, RunnerError> {
        if self.argv.is_empty()
            || self.argv.len() > eggwork_core::MAX_ARG_COUNT
            || self
                .argv
                .iter()
                .any(|a| a.is_empty() || a.len() > eggwork_core::MAX_ARG_BYTES || a.contains('\0'))
        {
            return Err(RunnerError::InvalidRequest(
                "argv is empty or contains an empty/NUL entry".into(),
            ));
        }
        if self.environment.len() > MAX_ENV_COUNT
            || self.environment.iter().any(|(k, v)| {
                k.is_empty()
                    || k.len() > MAX_ENV_NAME_BYTES
                    || v.len() > MAX_ENV_VALUE_BYTES
                    || k.contains('\0')
                    || k.contains('=')
                    || v.contains('\0')
            })
        {
            return Err(RunnerError::InvalidRequest(
                "environment violates bounds or syntax".into(),
            ));
        }
        if self.capture_limit > MAX_CAPTURE_BYTES
            || self.event_chunk_bytes == 0
            || self.event_chunk_bytes > MAX_EVENT_CHUNK_BYTES
        {
            return Err(RunnerError::InvalidRequest(
                "output limits are outside supported bounds".into(),
            ));
        }
        if self.timeout.is_zero() || self.timeout > eggwork_core::MAX_TIMEOUT {
            return Err(RunnerError::InvalidRequest(
                "timeout must be positive".into(),
            ));
        }
        self.stdin
            .validate()
            .map_err(|e| RunnerError::InvalidRequest(e.to_string()))?;
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

#[derive(Clone, PartialEq, Eq)]
pub enum StdinPolicy {
    Null,
    Bytes(Vec<u8>),
}
impl std::fmt::Debug for StdinPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => f.write_str("Null"),
            Self::Bytes(bytes) => f
                .debug_struct("Bytes")
                .field("length", &bytes.len())
                .field("value", &"[REDACTED]")
                .finish(),
        }
    }
}
impl StdinPolicy {
    fn validate(&self) -> Result<(), RunnerError> {
        if matches!(self, Self::Bytes(v) if v.len() > eggwork_core::MAX_STDIN_BYTES) {
            return Err(RunnerError::InvalidRequest("stdin exceeds limit".into()));
        }
        Ok(())
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
    #[error("invalid execution request: {0}")]
    InvalidRequest(String),
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

    pub async fn resource_capabilities(&self) -> Vec<String> {
        self.setup.resource_capabilities().await
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
        #[cfg(not(unix))]
        {
            let _ = (cwd, request, output_tx);
            return Err(RunnerError::UnsupportedPlatform);
        }
        #[cfg(unix)]
        {
            let mut setup = self
                .setup
                .prepare(&request.sandbox, &request.resources, &request.root)
                .await?;
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
                        None
                    }
                    Err(error) => return Err(error),
                }
            } else {
                None
            };
            #[cfg(not(target_os = "linux"))]
            let channel: Option<()> = None;
            #[cfg(target_os = "linux")]
            let command = if let Some(channel) = channel.as_ref() {
                channel.command()
            } else {
                direct_command(&request, cwd.clone())
            };
            #[cfg(not(target_os = "linux"))]
            let mut command = direct_command(&request, cwd.clone());
            #[cfg(target_os = "linux")]
            let (mut command, mut resource_unit) =
                wrap_resource_command(command, &request.resources, &setup.resources)?;
            #[cfg(target_os = "linux")]
            let mut sandbox_session = None;
            #[cfg(not(target_os = "linux"))]
            let resource_unit = None;
            #[cfg(not(target_os = "linux"))]
            let mut command = command;
            command.process_group(0);
            let mut child = command
                .spawn()
                .map_err(|e| RunnerError::Spawn(e.to_string()))?;
            #[cfg(target_os = "linux")]
            if let Some(sandbox_channel) = channel.take() {
                let handshake = sandbox_channel.wait(&mut child, cancellation.clone()).await;
                match handshake {
                    Ok(Some(session)) => sandbox_session = Some(session),
                    Ok(None)
                        if !matches!(request.sandbox, SandboxRequest::Required { .. })
                            && !request.resources.has_required() =>
                    {
                        let _ = child.wait().await;
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
                        let direct = direct_command(&request, cwd.clone());
                        let (mut direct, unit) =
                            wrap_resource_command(direct, &request.resources, &setup.resources)?;
                        resource_unit = unit;
                        direct.process_group(0);
                        child = direct
                            .spawn()
                            .map_err(|error| RunnerError::Spawn(error.to_string()))?;
                    }
                    Ok(None) => {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        return Err(RunnerError::RequiredSetupUnavailable);
                    }
                    Err(SandboxWaitError::BeforeTarget(error))
                        if !matches!(request.sandbox, SandboxRequest::Required { .. })
                            && !request.resources.has_required()
                            && !matches!(error, RunnerError::CancelledBeforeSpawn) =>
                    {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
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
                        let direct = direct_command(&request, cwd.clone());
                        let (mut direct, unit) =
                            wrap_resource_command(direct, &request.resources, &setup.resources)?;
                        resource_unit = unit;
                        direct.process_group(0);
                        child = direct
                            .spawn()
                            .map_err(|spawn_error| RunnerError::Spawn(spawn_error.to_string()))?;
                    }
                    Err(error) => {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        return Err(error.into_runner_error());
                    }
                }
            }
            let process_group = child.id();
            let stdin_task = match (child.stdin.take(), &request.stdin) {
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
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| RunnerError::Io("stdout pipe missing".into()))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| RunnerError::Io("stderr pipe missing".into()))?;
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
                overflow_tx,
                request.overflow,
            );
            let deadline = request.timeout;
            let (termination, status, wait_error) = tokio::select! {
                biased;
                _ = cancellation.cancelled() => (TerminationReason::Cancelled, None, None),
                _ = sleep(deadline) => (TerminationReason::TimedOut, None, None),
                changed = overflow_rx.changed() => {
                    if changed.is_ok() && *overflow_rx.borrow() { (TerminationReason::OutputLimit, None, None) }
                    else { (TerminationReason::Exited, None, Some("output monitor closed".into())) }
                }
                result = child.wait() => match result {
                    Ok(status) => (TerminationReason::Exited, Some(status), None),
                    Err(error) => (TerminationReason::Exited, None, Some(error.to_string())),
                }
            };
            #[cfg(target_os = "linux")]
            let mut resource_limits_exceeded = if matches!(&termination, TerminationReason::Exited)
                && let Some(session) = sandbox_session.take()
            {
                session.resource_limits_exceeded().await
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
            let status = if let Some(status) = status {
                // A command can leave background descendants behind and exit
                // while they still hold the output pipes. Reap that process
                // group before returning, without changing the leader's
                // already-known exit classification.
                match terminate_group(process_group) {
                    Ok(true) => {
                        sleep(TERMINATION_GRACE).await;
                        if let Err(error) = kill_group(process_group) {
                            cleanup.process_group_signal_error = Some(error);
                        }
                    }
                    Ok(false) => {}
                    Err(error) => cleanup.process_group_signal_error = Some(error),
                }
                Some(status)
            } else {
                match terminate_group(process_group) {
                    Ok(true) => sleep(TERMINATION_GRACE).await,
                    Ok(false) => {}
                    Err(error) => cleanup.process_group_signal_error = Some(error),
                }
                if let Err(error) = kill_group(process_group) {
                    cleanup.process_group_signal_error.get_or_insert(error);
                }
                match timeout(TERMINATION_GRACE, child.wait()).await {
                    Ok(Ok(status)) => Some(status),
                    Ok(Err(error)) => {
                        cleanup.wait_error = Some(error.to_string());
                        None
                    }
                    Err(_) => {
                        cleanup.wait_error = Some("child did not exit after SIGKILL".into());
                        None
                    }
                }
            };
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
}

fn denied_environment_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    [
        "LD_",
        "DYLD_",
        "PATH",
        "IFS",
        "SHELLOPTS",
        "CDPATH",
        "BASH_ENV",
        "ENV",
        "PYTHONPATH",
        "PYTHONHOME",
        "NODE_OPTIONS",
        "RUBYOPT",
        "PERL5OPT",
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GIT_CONFIG",
        "GIT_CONFIG_",
        "AWS_",
        "AZURE_",
        "GOOGLE_",
        "SSH_AUTH_SOCK",
    ]
    .iter()
    .any(|denied| upper == *denied || upper.starts_with(denied))
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
    command
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PAGER", "cat");
    for (key, value) in &request.environment {
        if !denied_environment_key(key) {
            command.env(key, value);
        }
    }
    if let Some(id) = &request.provenance.execution_id {
        command.env("EGGWORK_EXECUTION_ID", id.as_str());
    }
    if let Some(generation) = request.provenance.generation {
        command.env("EGGWORK_EXECUTION_GENERATION", generation.get().to_string());
    }
    command
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
    async fn resource_limits_exceeded(mut self) -> Vec<eggwork_core::ResourceDimension> {
        let mut status = [0u8; 1];
        if !matches!(
            timeout(Duration::from_secs(2), self.stream.read_exact(&mut status)).await,
            Ok(Ok(_))
        ) {
            return Vec::new();
        }
        let mut exceeded = Vec::new();
        if status[0] & 1 != 0 {
            exceeded.push(eggwork_core::ResourceDimension::Memory);
        }
        if status[0] & 2 != 0 {
            exceeded.push(eggwork_core::ResourceDimension::Pids);
        }
        exceeded
    }
}

#[cfg(target_os = "linux")]
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
                if profile == "workspace_rw" =>
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
        let mut environment = vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("CI".into(), "1".into()),
            ("NO_COLOR".into(), "1".into()),
            ("TERM".into(), "dumb".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("PAGER".into(), "cat".into()),
        ];
        for (key, value) in &request.environment {
            if !denied_environment_key(key) {
                environment.push((key.clone(), value.clone()));
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
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8");
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
                match status[0] {
                    0 | 2 | 3 | 30 | 31 => Ok(None),
                    code if code >= 4 => Err(SandboxWaitError::BeforeTarget(RunnerError::Setup(format!("sandbox helper rejected its launch specification (status {code})")))),
                    1 => {
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
                    _ => Err(SandboxWaitError::BeforeTarget(RunnerError::Setup("sandbox status was invalid".into()))),
                }
            }
        }
    }
}

#[cfg(unix)]
fn terminate_group(pid: Option<u32>) -> Result<bool, String> {
    let pid = pid.ok_or_else(|| "child pid is unavailable".to_owned())?;
    match nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGTERM,
    ) {
        Ok(()) => Ok(true),
        Err(nix::errno::Errno::ESRCH) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}
#[cfg(unix)]
fn kill_group(pid: Option<u32>) -> Result<(), String> {
    let pid = pid.ok_or_else(|| "child pid is unavailable".to_owned())?;
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .or_else(|error| {
        if error == nix::errno::Errno::ESRCH {
            Ok(())
        } else {
            Err(error)
        }
    })
    .map_err(|e| e.to_string())
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
                if output_tx
                    .try_send(OutputChunk {
                        stderr,
                        bytes: bytes.to_vec(),
                    })
                    .is_err()
                {
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

    #[tokio::test]
    async fn unsupported_resource_limits_are_reported_or_rejected_before_spawn() {
        let temp = tempfile::tempdir().unwrap();
        let runner = LocalProcessRunner::new(NoExecutionSetup);
        let mut best_effort = request(temp.path(), &["/bin/true"]);
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

        let marker = temp.path().join("required-limit-target-ran");
        let mut required = request(temp.path(), &["/usr/bin/touch", marker.to_str().unwrap()]);
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
        assert_eq!(result.stdout.head, b"input:unset");
        assert_eq!(result.stderr.head, b"err");
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

    #[cfg(unix)]
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
        assert!(matches!(
            run(request(
                temp.path(),
                &["/definitely/missing/eggwork-command"]
            ))
            .await,
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
        assert_eq!(
            String::from_utf8_lossy(&result.stdout.head).trim(),
            temp.path().join("work").to_string_lossy()
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
        let proc_state = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        assert!(proc_state.is_err() || proc_state.unwrap().split_whitespace().nth(2) == Some("Z"));
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
}
