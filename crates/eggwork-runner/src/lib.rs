#![forbid(unsafe_code)]

//! Canonical owner of finite, noninteractive process execution.

use eggwork_core::{
    CommandSpec, ExecutionFailure, ExecutionGeneration, ExecutionId, ExecutionResult,
    ExecutionSpec, ExecutionState, MAX_CAPTURE_BYTES, MAX_ENV_COUNT, MAX_ENV_NAME_BYTES,
    MAX_ENV_VALUE_BYTES, MAX_EVENT_CHUNK_BYTES, OverflowPolicy as CoreOverflowPolicy, RelativePath,
    StdinPolicy as CoreStdinPolicy,
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

/// Requested process-tree behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTreePolicy {
    /// Require a host process-group primitive that can terminate descendants.
    Required,
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
    pub memory_bytes: Option<u64>,
    pub cpu_millis: Option<u64>,
    pub pids: Option<u32>,
    pub required: bool,
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
        if resources.required {
            return Err(RunnerError::RequiredSetupUnavailable);
        }
        let resource_outcome = if resources.memory_bytes.is_some()
            || resources.cpu_millis.is_some()
            || resources.pids.is_some()
        {
            ResourceSetupOutcome::NotApplied {
                reason: "no resource-control backend is configured".into(),
            }
        } else {
            ResourceSetupOutcome::NotRequested
        };
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
        if matches!(sandbox, SandboxRequest::Required { .. }) || resources.required {
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
            resources: if resources.memory_bytes.is_some()
                || resources.cpu_millis.is_some()
                || resources.pids.is_some()
            {
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
    pub argv: Vec<String>,
    pub root: PathBuf,
    pub cwd: Option<RelativePath>,
    pub environment: Vec<(String, String)>,
    pub stdin: StdinPolicy,
    pub timeout: Duration,
    pub capture_limit: usize,
    pub event_chunk_bytes: usize,
    pub overflow: CoreOverflowPolicy,
    pub provenance: ExecutionProvenance,
    pub sandbox: SandboxRequest,
    pub resources: ResourceSetupRequest,
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
                required: false,
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
    pub stream_chunks_dropped: u64,
    pub provenance: ExecutionProvenance,
}

impl RunnerResult {
    pub fn execution_result(&self) -> ExecutionResult {
        let state = match self.termination {
            TerminationReason::Exited if self.exit_code == Some(0) => ExecutionState::Succeeded,
            TerminationReason::Exited => ExecutionState::Failed,
            TerminationReason::TimedOut => ExecutionState::TimedOut,
            TerminationReason::Cancelled => ExecutionState::Cancelled,
            TerminationReason::OutputLimit => ExecutionState::Failed,
        };
        let failure = match self.termination {
            TerminationReason::OutputLimit => Some(ExecutionFailure::OutputLimit),
            _ if self.exit_code.is_none()
                && matches!(self.termination, TerminationReason::Exited) =>
            {
                Some(ExecutionFailure::Internal)
            }
            _ => None,
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
        }
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
            let use_helper = !matches!(request.sandbox, SandboxRequest::None)
                && matches!(setup.sandbox, SandboxOutcome::Applied { .. })
                && self.setup.sandbox_helper_path().is_some();
            #[cfg(target_os = "linux")]
            let mut channel = if use_helper {
                match SandboxChannel::new(
                    self.setup.sandbox_helper_path().expect("checked above"),
                    &request,
                    cwd.clone(),
                ) {
                    Ok(channel) => Some(channel),
                    Err(error) if matches!(request.sandbox, SandboxRequest::BestEffort { .. }) => {
                        setup.sandbox = SandboxOutcome::NotApplied {
                            reason: error.to_string(),
                        };
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
            let mut command = if let Some(channel) = channel.as_ref() {
                channel.command()
            } else {
                direct_command(&request, cwd.clone())
            };
            #[cfg(not(target_os = "linux"))]
            let mut command = direct_command(&request, cwd.clone());
            command.process_group(0);
            let mut child = command
                .spawn()
                .map_err(|e| RunnerError::Spawn(e.to_string()))?;
            #[cfg(target_os = "linux")]
            if let Some(sandbox_channel) = channel.take() {
                let handshake = sandbox_channel.wait(&mut child, cancellation.clone()).await;
                match handshake {
                    Ok(true) => {}
                    Ok(false) if matches!(request.sandbox, SandboxRequest::BestEffort { .. }) => {
                        let _ = child.wait().await;
                        setup.sandbox = SandboxOutcome::NotApplied {
                            reason: "Landlock helper could not enforce the requested profile"
                                .into(),
                        };
                        let mut direct = direct_command(&request, cwd.clone());
                        direct.process_group(0);
                        child = direct
                            .spawn()
                            .map_err(|error| RunnerError::Spawn(error.to_string()))?;
                    }
                    Ok(false) => {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        return Err(RunnerError::RequiredSetupUnavailable);
                    }
                    Err(SandboxWaitError::BeforeTarget(error))
                        if matches!(request.sandbox, SandboxRequest::BestEffort { .. })
                            && !matches!(error, RunnerError::CancelledBeforeSpawn) =>
                    {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        setup.sandbox = SandboxOutcome::NotApplied {
                            reason: error.to_string(),
                        };
                        let mut direct = direct_command(&request, cwd.clone());
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
            let mut cleanup = CleanupDiagnostics {
                process_group_signal_error: None,
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
#[derive(Serialize)]
struct SandboxLaunchSpec {
    schema_version: u16,
    profile: String,
    root: PathBuf,
    cwd: PathBuf,
    argv: Vec<String>,
    environment: Vec<(String, String)>,
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
    fn new(helper_path: &Path, request: &RunnerRequest, cwd: PathBuf) -> Result<Self, RunnerError> {
        let profile = match &request.sandbox {
            SandboxRequest::BestEffort { profile } | SandboxRequest::Required { profile }
                if profile == "workspace_rw" =>
            {
                profile.clone()
            }
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
    ) -> Result<bool, SandboxWaitError> {
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
                    0 => Ok(false),
                    2 => Ok(false),
                    3 => Ok(false),
                    30 => Ok(false),
                    31 => Ok(false),
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
                            1 => Ok(true),
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
                memory_bytes: None,
                cpu_millis: None,
                pids: None,
                required: false,
            },
        }
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
