//! Consumer deployment and service lifecycle backed by Eggup (Operations M002).
//!
//! Ownership boundary: Eggup owns filesystem transaction/rollback mechanics,
//! install receipts, and platform service-manager adapters. Eggwork owns the
//! install-unit policy (which artifacts form one coherent node installation),
//! the decision that remote execution must drain before replacement, helper
//! compatibility policy, and bounded post-update health validation.
//!
//! Eggwork never invokes a platform service manager directly and never
//! implements file backup/restoration itself. All `systemctl`, `launchctl`,
//! SCM, and `crontab` interaction goes through `eggup-service` adapters.

use std::{
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

/// Consumer product identity for the installed Eggwork node.
pub const PRODUCT_ID: &str = "eggwork";
/// Stable service identity for the installed node daemon.
pub const SERVICE_ID: &str = "eggwork-node";
/// Artifact member identity for the daemon binary.
pub const MEMBER_DAEMON: &str = "eggworkd";
/// Artifact member identity for the Linux sandbox helper.
pub const MEMBER_HELPER: &str = "eggwork-sandbox-helper";
/// Destination of the daemon relative to the installation root.
pub const DEST_DAEMON: &str = "bin/eggworkd";
/// Destination of the helper relative to the installation root.
pub const DEST_HELPER: &str = "bin/eggwork-sandbox-helper";
/// Pinned Eggup consumer dependency qualified by this milestone.
pub const EGGUP_CORE_VERSION: &str = "0.1.1";
/// Pinned Eggup service dependency qualified by this milestone.
pub const EGGUP_SERVICE_VERSION: &str = "0.1.1";
/// Maximum candidate source bytes accepted for staging (bounded).
pub const MAX_CANDIDATE_BYTES: u64 = 256 * 1024 * 1024;
/// Default bounded drain wait before an update proceeds or fails closed.
pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
/// Default bounded post-update health probe budget.
pub const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
/// Quiescence poll interval.
///
/// This used to be 5 ms, which over a 30 s drain window meant up to 6,000 polls.
/// Each poll opened its own read-only SQLite connection, so a single update could
/// spend minutes of I/O just watching for quiescence. The probe is now a single
/// indexed `COUNT(*)`, and 100 ms keeps drain latency well under a tenth of the
/// timeout while cutting the poll count ~20x. The granularity was never a
/// correctness property — the loop is bounded by `drain_timeout` either way.
pub const QUIESCENCE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Secret-safe deployment error with bounded detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentError {
    kind: &'static str,
    detail: String,
}

impl DeploymentError {
    fn new(kind: &'static str, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let bounded = if detail.len() > 256 {
            detail.chars().take(256).collect()
        } else {
            detail
        };
        Self {
            kind,
            detail: bounded,
        }
    }

    /// Machine-readable error kind.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Record, on a failure, that the node was left refusing new work.
    ///
    /// The drain marker is set *before* quiescence and is deliberately never
    /// cleared on any error path — an update that dies mid-flight must not let
    /// the node resume accepting executions against a half-installed release.
    /// That is the right behaviour, but it used to be invisible: only the
    /// success report carried `drain_remains_active`, so an operator reading a
    /// failure had no way to learn that a later `eggworkd undrain` was needed.
    fn note_drain_remains(self, drain_remains_active: bool) -> Self {
        const SUFFIX: &str = "; node left draining, run `eggworkd undrain` to clear";
        if !drain_remains_active || self.detail.contains(SUFFIX) {
            return self;
        }
        // Reserve room for the suffix rather than truncating it away.
        let room = 256usize.saturating_sub(SUFFIX.chars().count());
        let mut detail = self.detail.chars().take(room).collect::<String>();
        detail.push_str(SUFFIX);
        Self {
            kind: self.kind,
            detail,
        }
    }
}

impl fmt::Display for DeploymentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "deployment {}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for DeploymentError {}

impl From<eggup_service::LifecycleUpdateError> for DeploymentError {
    fn from(error: eggup_service::LifecycleUpdateError) -> Self {
        Self::new("lifecycle", truncate(&error.to_string()))
    }
}

impl From<eggup_core::Error> for DeploymentError {
    fn from(error: eggup_core::Error) -> Self {
        Self::new("eggup", truncate(&error.to_string()))
    }
}

impl From<eggup_service::ServiceError> for DeploymentError {
    fn from(error: eggup_service::ServiceError) -> Self {
        Self::new("service", truncate(&error.to_string()))
    }
}

fn truncate(text: &str) -> String {
    if text.len() > 256 {
        text.chars().take(256).collect()
    } else {
        text.to_owned()
    }
}

/// One member of the consumer install unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallMember {
    /// Stable Eggup member identity.
    pub member: String,
    /// Destination relative to the installation root.
    pub destination: String,
    /// Whether the member is required on this platform.
    pub required: bool,
}

/// The consumer deployment unit: the binaries that must move together inside
/// one Eggup multi-artifact transaction so helper/executable versions cannot
/// drift independently.
pub fn install_unit_matrix(helper_required: bool) -> Vec<InstallMember> {
    vec![
        InstallMember {
            member: MEMBER_DAEMON.into(),
            destination: DEST_DAEMON.into(),
            required: true,
        },
        InstallMember {
            member: MEMBER_HELPER.into(),
            destination: DEST_HELPER.into(),
            required: helper_required,
        },
    ]
}

/// Build the stable `ServiceSpec` for an installed daemon.
///
/// Ownership identity includes the exact installed executable, the critical
/// `run` argv marker, and the config path (both argv and config participate
/// so same-name/different-executable or same-name/different-config can never
/// compare as owned).
pub fn service_spec_for_install(
    executable: &Path,
    config_path: Option<&Path>,
) -> Result<eggup_service::ServiceSpec, DeploymentError> {
    if !executable.is_absolute() {
        return Err(DeploymentError::new(
            "invalid",
            "daemon executable must be absolute",
        ));
    }
    let id = eggup_service::ServiceId::new(SERVICE_ID)
        .map_err(|_| DeploymentError::new("invalid", "service identity is invalid"))?;
    let mut args = vec!["run".to_owned()];
    let mut config = None;
    if let Some(path) = config_path {
        if !path.is_absolute() {
            return Err(DeploymentError::new(
                "invalid",
                "config path must be absolute",
            ));
        }
        args.push("--config".to_owned());
        args.push(path.display().to_string());
        config = Some(path.to_path_buf());
    }
    eggup_service::ServiceSpec::new(id, executable.to_path_buf(), args, config)
        .map_err(DeploymentError::from)
}

/// Render caller-owned systemd unit bytes for the installed daemon.
///
/// The bytes are caller-owned content supplied to `SystemdInstall`; Eggwork
/// never writes unit files itself.
pub fn render_systemd_unit(
    executable: &Path,
    config_path: &Path,
) -> Result<Vec<u8>, DeploymentError> {
    if !executable.is_absolute() || !config_path.is_absolute() {
        return Err(DeploymentError::new(
            "invalid",
            "unit paths must be absolute",
        ));
    }
    for path in [executable, config_path] {
        let text = path.display().to_string();
        // systemd ExecStart has quoting, escaping, specifier, and variable
        // expansion rules. Accept only a literal path alphabet so the
        // rendered command cannot diverge from ServiceSpec argv.
        if !text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.+".contains(&byte))
        {
            return Err(DeploymentError::new(
                "invalid",
                "unit paths contain characters unsupported by ExecStart policy",
            ));
        }
    }
    Ok(format!(
        "[Unit]\nDescription=Eggwork node daemon\nAfter=network.target\n\n[Service]\nType=simple\nExecStart={} run --config {}\nRestart=on-failure\nRestartSec=2\nNoNewPrivileges=true\n\n[Install]\nWantedBy=multi-user.target\n",
        executable.display(),
        config_path.display()
    )
    .into_bytes())
}

/// Outcome of reading the installed daemon's own version report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstalledVersionCheck {
    /// The installed daemon reports exactly the expected release version.
    Matched { version: String },
    /// The probe ran but the reported version is not the expected release.
    Mismatch { reported: String },
    /// The probe produced no usable version report.
    Unreadable { reason: &'static str },
}

impl InstalledVersionCheck {
    /// Whether required post-install validation may proceed.
    pub fn matched(&self) -> bool {
        matches!(self, Self::Matched { .. })
    }
}

/// Maximum bytes accepted from an installed daemon's `version` probe.
pub const MAX_VERSION_PROBE_BYTES: usize = 1024;

/// Decide whether an installed daemon reports the expected release version.
///
/// The daemon answers `version` with a JSON object (`{"version": "..."}`), not a
/// bare string, so a raw string comparison would fail every real update. This
/// reads the same bounded, shell-free probe output and requires the reported
/// version field to equal the expected release exactly.
///
/// The comparison is exact on purpose: a build that reports a different version
/// is a different release generation, and admitting it would let a post-install
/// check pass for a binary the operator did not stage.
pub fn check_installed_daemon_version(stdout: &[u8], expected: &str) -> InstalledVersionCheck {
    if stdout.len() > MAX_VERSION_PROBE_BYTES {
        return InstalledVersionCheck::Unreadable {
            reason: "version probe output exceeded its bound",
        };
    }
    let text = match std::str::from_utf8(stdout) {
        Ok(text) => text.trim(),
        Err(_) => {
            return InstalledVersionCheck::Unreadable {
                reason: "version probe output is not UTF-8",
            };
        }
    };
    let report: serde_json::Value = match serde_json::from_str(text) {
        Ok(report) => report,
        Err(_) => {
            return InstalledVersionCheck::Unreadable {
                reason: "version probe output is not a JSON object",
            };
        }
    };
    let reported = report.get("version").and_then(|value| value.as_str());
    match reported {
        Some(reported) if reported == expected => InstalledVersionCheck::Matched {
            version: reported.to_owned(),
        },
        Some(reported) => InstalledVersionCheck::Mismatch {
            reported: reported.to_owned(),
        },
        None => InstalledVersionCheck::Unreadable {
            reason: "version probe output has no version field",
        },
    }
}

/// Helper compatibility outcome for required sandbox execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperCompatibility {
    /// Helper is present, trusted, and version-coherent.
    Compatible { version: String },
    /// No helper is configured (required isolation fails closed elsewhere).
    NotConfigured,
    /// Helper binary is missing at the configured path.
    Missing,
    /// Helper fails installation ownership/mode checks.
    Untrusted { reason: String },
    /// Helper reports a different version than the daemon release.
    VersionSkew { expected: String, found: String },
}

impl HelperCompatibility {
    /// Whether required sandbox execution may proceed.
    pub fn compatible(&self) -> bool {
        matches!(self, Self::Compatible { .. })
    }
}

/// Whether required filesystem isolation may be admitted given helper trust.
///
/// Only `Compatible` admits required isolation. Every other outcome —
/// including `NotConfigured` — fails closed and must surface as
/// `capability_mismatch` before target spawn, never as best-effort downgrade.
pub fn helper_satisfies_required_isolation(compatibility: &HelperCompatibility) -> bool {
    compatibility.compatible()
}

/// Check helper presence, installation trust, and version coherence.
///
/// Version coherence is established by invoking `<helper> --version` in a
/// bounded child process and comparing the trimmed stdout against the
/// expected daemon version. Any spawn failure, timeout, overflow, or mismatch
/// is fail-closed (never treated as compatible).
pub fn check_helper_compatibility(
    helper_path: Option<&Path>,
    expected_version: &str,
) -> HelperCompatibility {
    check_helper_compatibility_with_timeout(helper_path, expected_version, Duration::from_secs(5))
}

/// Check helper trust/version within a caller-supplied bounded child timeout.
pub fn check_helper_compatibility_with_timeout(
    helper_path: Option<&Path>,
    expected_version: &str,
    timeout: Duration,
) -> HelperCompatibility {
    let Some(path) = helper_path else {
        return HelperCompatibility::NotConfigured;
    };
    if timeout.is_zero() {
        return HelperCompatibility::Untrusted {
            reason: "helper version probe budget is exhausted".into(),
        };
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return HelperCompatibility::Missing;
        }
        Err(_) => {
            return HelperCompatibility::Untrusted {
                reason: "helper metadata is unreadable".into(),
            };
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return HelperCompatibility::Untrusted {
            reason: "helper must be a regular file".into(),
        };
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode();
        let effective_uid = rustix::process::geteuid().as_raw();
        if (metadata.uid() != 0 && metadata.uid() != effective_uid) || mode & 0o022 != 0 {
            return HelperCompatibility::Untrusted {
                reason: "helper ownership or mode check failed".into(),
            };
        }
        if mode & 0o111 == 0 {
            return HelperCompatibility::Untrusted {
                reason: "helper is not executable".into(),
            };
        }
    }
    let program: PathBuf = path.to_path_buf();
    let spec = eggup_core::CommandSpec::new(program)
        .arg("--version")
        .timeout(timeout);
    let output = match eggup_core::run_bounded(&spec) {
        Ok(output) => output,
        Err(_) => {
            return HelperCompatibility::Untrusted {
                reason: "helper version probe failed".into(),
            };
        }
    };
    if !output.success() {
        return HelperCompatibility::Untrusted {
            reason: "helper version probe reported failure".into(),
        };
    }
    let found = String::from_utf8_lossy(output.stdout()).trim().to_owned();
    if found.is_empty() || found.len() > 64 || found.chars().any(char::is_control) {
        return HelperCompatibility::Untrusted {
            reason: "helper version output is invalid".into(),
        };
    }
    if found != expected_version {
        return HelperCompatibility::VersionSkew {
            expected: expected_version.to_owned(),
            found,
        };
    }
    HelperCompatibility::Compatible { version: found }
}

/// Bounded update policy owned by Eggwork (application decisions only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdatePolicy {
    /// How long to wait for active executions to finish before failing closed.
    pub drain_timeout: Duration,
    /// Budget for the post-update health/version probe (informational; the
    /// Eggup post-commit callback itself is bounded by the probe).
    pub health_timeout: Duration,
    /// When true, proceed even with active executions (explicit operator
    /// choice; active work is still never fabricated as complete).
    pub force: bool,
    /// What Eggup must do when the post-commit health check fails.
    pub post_commit: eggup_core::PostCommitFailurePolicy,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            drain_timeout: DEFAULT_DRAIN_TIMEOUT,
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            force: false,
            post_commit: eggup_core::PostCommitFailurePolicy::RollBack,
        }
    }
}

/// Wait until no active executions remain, honoring the bounded policy.
///
/// `active` reports the current number of non-terminal executions. Without
/// `force`, a timeout is fail-closed. With `force`, the caller explicitly
/// accepts proceeding while work remains (the work itself is cancelled
/// through the normal shutdown path, never fabricated as complete).
pub fn wait_for_quiescence(
    active: impl Fn() -> usize,
    policy: UpdatePolicy,
) -> Result<(), DeploymentError> {
    if policy.force {
        return Ok(());
    }
    let start = Instant::now();
    loop {
        if active() == 0 {
            return Ok(());
        }
        if start.elapsed() >= policy.drain_timeout {
            return Err(DeploymentError::new(
                "draining",
                "active executions remain after bounded drain wait",
            ));
        }
        std::thread::sleep(QUIESCENCE_POLL_INTERVAL);
    }
}

/// One candidate artifact staged outside the installation root.
#[derive(Debug, Clone)]
pub struct CandidateSource {
    /// Stable member identity (`eggworkd` / `eggwork-sandbox-helper`).
    pub member: String,
    /// Absolute source file acquired from the release channel.
    pub source: PathBuf,
    /// Destination relative to the installation root.
    pub destination: String,
}

impl CandidateSource {
    /// Validate bounds: known member, absolute source, relative destination.
    pub fn validate(&self) -> Result<(), DeploymentError> {
        if self.member != MEMBER_DAEMON && self.member != MEMBER_HELPER {
            return Err(DeploymentError::new(
                "candidate",
                "unknown candidate member",
            ));
        }
        if !self.source.is_absolute() {
            return Err(DeploymentError::new(
                "candidate",
                "candidate source must be absolute",
            ));
        }
        if self.destination.is_empty()
            || Path::new(&self.destination).is_absolute()
            || self.destination.contains("..")
        {
            return Err(DeploymentError::new(
                "candidate",
                "candidate destination is invalid",
            ));
        }
        let metadata = std::fs::symlink_metadata(&self.source)
            .map_err(|_| DeploymentError::new("candidate", "candidate source is unreadable"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(DeploymentError::new(
                "candidate",
                "candidate source must be a regular file",
            ));
        }
        if metadata.len() == 0 || metadata.len() > MAX_CANDIDATE_BYTES {
            return Err(DeploymentError::new(
                "candidate",
                "candidate source size is out of bounds",
            ));
        }
        Ok(())
    }
}

/// Build a validated `InstallPlan` with SHA-256 integrity attached per member.
///
/// Integrity digests are computed from the staged sources so the later
/// `verify_integrity` gate proves the exact bytes committed.
pub fn build_install_plan(
    installation_root: &Path,
    release: &str,
    sources: &[CandidateSource],
) -> Result<eggup_core::InstallPlan, DeploymentError> {
    if sources.is_empty() {
        return Err(DeploymentError::new("candidate", "candidate set is empty"));
    }
    let product = eggup_core::ProductId::new(PRODUCT_ID.to_owned())
        .map_err(|_| DeploymentError::new("candidate", "product identity is invalid"))?;
    let release_id = eggup_core::ReleaseId::new(release.to_owned())
        .map_err(|_| DeploymentError::new("candidate", "release identity is invalid"))?;
    let mut members = Vec::with_capacity(sources.len());
    for source in sources {
        source.validate()?;
        let digest = eggup_core::hash_file(&source.source)?;
        let member_id = eggup_core::MemberId::new(source.member.clone())
            .map_err(|_| DeploymentError::new("candidate", "member identity is invalid"))?;
        let member = eggup_core::ArtifactMember::new(
            member_id,
            source.source.clone(),
            PathBuf::from(&source.destination),
        )?
        .with_integrity(eggup_core::IntegrityRequirement::Sha256(digest));
        members.push(member);
    }
    let set = eggup_core::ArtifactSet::new(members)?;
    eggup_core::InstallPlan::new(product, release_id, installation_root, set)
        .map_err(DeploymentError::from)
}

/// Commit a prepared plan through Eggup with a bounded post-commit health
/// check running while backups remain retained.
///
/// `health_check` runs after all members are live and before backups are
/// discarded. On failure, `policy` selects `RollBack` (restore and verify the
/// previous generation) or `KeepInstalled` (explicit operator policy only).
pub fn commit_with_health_check(
    plan: eggup_core::InstallPlan,
    verifier: &dyn eggup_core::OwnershipVerifier,
    absent: eggup_core::AbsentPolicy,
    policy: eggup_core::PostCommitFailurePolicy,
    health_check: impl FnOnce() -> Result<(), String>,
) -> Result<eggup_core::TransactionReceipt, DeploymentError> {
    let prepared = plan.prepare()?;
    let verified = prepared.verify_integrity()?;
    let validated = verified.validate(&eggup_core::AllValidators::new())?;
    validated
        .commit_with_post_commit(
            eggup_core::CommitOwnership::new(verifier, absent),
            policy,
            health_check,
        )
        .map_err(DeploymentError::from)
}

/// Plain commit without a post-commit check (first install / tests only).
pub fn commit_without_health_check(
    plan: eggup_core::InstallPlan,
    verifier: &dyn eggup_core::OwnershipVerifier,
    absent: eggup_core::AbsentPolicy,
) -> Result<eggup_core::TransactionReceipt, DeploymentError> {
    let prepared = plan.prepare()?;
    let verified = prepared.verify_integrity()?;
    let validated = verified.validate(&eggup_core::AllValidators::new())?;
    validated
        .commit(eggup_core::CommitOwnership::new(verifier, absent))
        .map_err(DeploymentError::from)
}

/// Inspect service registration and lifecycle without mutating anything.
pub fn inspect_service(
    manager: &dyn eggup_service::ServiceManager,
    spec: &eggup_service::ServiceSpec,
) -> Result<eggup_service::LifecycleSnapshot, DeploymentError> {
    manager.inspect(spec).map_err(DeploymentError::from)
}

/// Thin orchestration: drain, stop owned service, transactional replace with
/// post-commit health validation, restore running state.
///
/// Each step fails closed:
/// - candidate validation happens before any service mutation;
/// - the persistent drain marker is set before replacement when a marker
///   path is supplied, so new execution admission stops while draining;
/// - only an `Owned` service is stopped or replaced;
/// - `Foreign`/`Unknown` registrations are never touched;
/// - active executions block the update unless `force` is explicit;
/// - post-commit health failure rolls back through Eggup (default) while
///   backups remain retained.
///
/// The Eggup transaction only replaces `bin/` members; execution recovery
/// state (database, workspaces, blobs, artifacts) is never part of the
/// install unit, so retained terminal records survive update/restart
/// truthfully and a failed update cannot fabricate execution completion.
pub struct UpdateRequest<'a> {
    /// Owned service identity to stop/replace/restart.
    pub spec: &'a eggup_service::ServiceSpec,
    /// Installation root whose `bin/` members are replaced.
    pub installation_root: &'a Path,
    /// Release identity for the candidate generation.
    pub release: &'a str,
    /// Validated candidate sources (validated again before any mutation).
    pub sources: &'a [CandidateSource],
    /// Persistent drain marker to set before replacement. When `None`, the
    /// caller (or a prior operator `drain` command) owns drain state and the
    /// orchestration only waits for quiescence.
    pub drain_marker: Option<&'a Path>,
    /// Bounded drain/health/force/post-commit policy.
    pub policy: UpdatePolicy,
}

pub fn orchestrate_update_with_lifecycle(
    manager: &mut dyn eggup_service::ServiceManager,
    request: UpdateRequest<'_>,
    verifier: &dyn eggup_core::OwnershipVerifier,
    active_executions: impl Fn() -> usize,
    health_check: impl FnOnce() -> Result<(), String>,
) -> Result<eggup_service::LifecycleUpdateReceipt, DeploymentError> {
    orchestrate_update_with_lifecycle_budgeted(
        manager,
        request,
        verifier,
        active_executions,
        move |_| health_check(),
    )
}

/// Lifecycle orchestration with a deadline-aware post-install callback.
/// Callers with bounded probes should use the supplied remaining budget.
pub fn orchestrate_update_with_lifecycle_budgeted(
    manager: &mut dyn eggup_service::ServiceManager,
    request: UpdateRequest<'_>,
    verifier: &dyn eggup_core::OwnershipVerifier,
    active_executions: impl Fn() -> usize,
    health_check: impl FnOnce(Duration) -> Result<(), String>,
) -> Result<eggup_service::LifecycleUpdateReceipt, DeploymentError> {
    for source in request.sources {
        source.validate()?;
    }
    let plan = build_install_plan(request.installation_root, request.release, request.sources)?;
    let validated = plan
        .prepare()?
        .verify_integrity()?
        .validate(&eggup_core::AllValidators::new())?;
    // Whether a failure below will leave this node refusing work. The marker is set
    // before quiescence and never cleared on an error path, which is deliberate —
    // but the operator has to be told, or a failed update looks like a clean one.
    let drain_remains_active = request.drain_marker.is_some();
    if let Some(marker) = request.drain_marker {
        crate::operations::set_persistent_drain(marker, true).map_err(|_| {
            DeploymentError::new("draining", "persistent drain marker could not be set")
        })?;
    }
    wait_for_quiescence(active_executions, request.policy)
        .map_err(|error| error.note_drain_remains(drain_remains_active))?;
    let mut adapter = ServiceManagerAdapter(manager);
    let check = OneShotHealthCheck(std::cell::RefCell::new(Some(health_check)));
    eggup_service::commit_with_lifecycle(
        &mut adapter,
        request.spec,
        validated,
        eggup_core::CommitOwnership::new(verifier, eggup_core::AbsentPolicy::AllowCreate),
        eggup_service::LifecycleUpdatePolicy {
            restore: eggup_service::RestoreIntent::Preserve,
            post_commit_failure: request.policy.post_commit,
            quiesce_timeout: Duration::from_secs(30),
            post_commit_timeout: request.policy.health_timeout,
            rollback_restore_timeout: Duration::from_secs(30),
        },
        &check,
    )
    .map_err(|error| DeploymentError::from(error).note_drain_remains(drain_remains_active))
}

pub fn orchestrate_update(
    manager: &mut dyn eggup_service::ServiceManager,
    request: UpdateRequest<'_>,
    verifier: &dyn eggup_core::OwnershipVerifier,
    active_executions: impl Fn() -> usize,
    health_check: impl FnOnce() -> Result<(), String>,
) -> Result<eggup_core::TransactionReceipt, DeploymentError> {
    orchestrate_update_with_lifecycle(manager, request, verifier, active_executions, health_check)
        .map(|receipt| receipt.transaction)
}

struct ServiceManagerAdapter<'a>(&'a mut dyn eggup_service::ServiceManager);

impl eggup_service::ServiceManager for ServiceManagerAdapter<'_> {
    fn inspect(
        &self,
        s: &eggup_service::ServiceSpec,
    ) -> Result<eggup_service::LifecycleSnapshot, eggup_service::ServiceError> {
        self.0.inspect(s)
    }
    fn install(
        &mut self,
        s: &eggup_service::ServiceSpec,
    ) -> Result<eggup_service::TransitionResult, eggup_service::ServiceError> {
        self.0.install(s)
    }
    fn start(
        &mut self,
        s: &eggup_service::ServiceSpec,
        t: Duration,
    ) -> Result<eggup_service::TransitionResult, eggup_service::ServiceError> {
        self.0.start(s, t)
    }
    fn stop(
        &mut self,
        s: &eggup_service::ServiceSpec,
        t: Duration,
    ) -> Result<eggup_service::TransitionResult, eggup_service::ServiceError> {
        self.0.stop(s, t)
    }
    fn restart(
        &mut self,
        s: &eggup_service::ServiceSpec,
        t: Duration,
    ) -> Result<eggup_service::TransitionResult, eggup_service::ServiceError> {
        self.0.restart(s, t)
    }
    fn uninstall(
        &mut self,
        s: &eggup_service::ServiceSpec,
    ) -> Result<eggup_service::TransitionResult, eggup_service::ServiceError> {
        self.0.uninstall(s)
    }
}

struct OneShotHealthCheck<F>(std::cell::RefCell<Option<F>>);
impl<F> fmt::Debug for OneShotHealthCheck<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OneShotHealthCheck")
    }
}
impl<F: FnOnce(Duration) -> Result<(), String>> eggup_service::PostInstallCheck
    for OneShotHealthCheck<F>
{
    fn check(
        &self,
        _: &eggup_service::ServiceSpec,
        _: &eggup_service::LifecycleSnapshot,
        remaining: Duration,
    ) -> Result<(), eggup_service::PostInstallCheckError> {
        if remaining.is_zero() {
            return Err(eggup_service::PostInstallCheckError::new(
                "health check time budget exhausted",
            ));
        }
        self.0.borrow_mut().take().ok_or_else(|| {
            eggup_service::PostInstallCheckError::new("health check already consumed")
        })?(remaining)
        .map_err(eggup_service::PostInstallCheckError::new)
    }
}

/// Observe host manager facts without choosing policy or mutating anything.
pub fn platform_support() -> Result<eggup_service::HostFacts, DeploymentError> {
    let executor = eggup_service::SystemExecutor::new();
    eggup_service::inspect_host(&executor).map_err(DeploymentError::from)
}

/// Parse an explicit systemd scope; scope is never guessed from EUID.
pub fn parse_systemd_scope(text: &str) -> Result<eggup_service::SystemdScope, DeploymentError> {
    match text {
        "system" => Ok(eggup_service::SystemdScope::System),
        "user" => Ok(eggup_service::SystemdScope::User),
        _ => Err(DeploymentError::new(
            "invalid",
            "scope must be system or user",
        )),
    }
}

/// Construct a systemd adapter from explicit caller-owned install material.
///
/// All manager interaction flows through the returned Eggup adapter; Eggwork
/// never shells out to `systemctl` itself.
#[allow(clippy::too_many_arguments)]
pub fn systemd_manager(
    unit_name: &str,
    scope: eggup_service::SystemdScope,
    unit_path: &Path,
    executable: &Path,
    config_path: &Path,
    enable: bool,
    reload: bool,
    transition_timeout: Duration,
) -> Result<eggup_service::SystemdManager<eggup_service::SystemExecutor>, DeploymentError> {
    if !unit_path.is_absolute() {
        return Err(DeploymentError::new(
            "invalid",
            "unit path must be absolute",
        ));
    }
    let definition = render_systemd_unit(executable, config_path)?;
    let install = eggup_service::SystemdInstall::new(
        unit_name.to_owned(),
        scope,
        unit_path.to_path_buf(),
        definition,
        enable,
        reload,
        transition_timeout,
    )?;
    Ok(eggup_service::SystemdManager::new(
        eggup_service::SystemExecutor::new(),
        install,
    ))
}

/// Render a deterministic launchd plist with argv represented as an XML
/// array. Caller paths are XML-escaped and control characters are rejected.
pub fn render_launchd_plist(
    label: &str,
    executable: &Path,
    config_path: &Path,
) -> Result<Vec<u8>, DeploymentError> {
    if label != SERVICE_ID || !executable.is_absolute() || !config_path.is_absolute() {
        return Err(DeploymentError::new(
            "invalid",
            "launchd identity and paths are invalid",
        ));
    }
    let executable = executable
        .to_str()
        .ok_or_else(|| DeploymentError::new("invalid", "launchd executable path is not UTF-8"))?;
    let config = config_path
        .to_str()
        .ok_or_else(|| DeploymentError::new("invalid", "launchd config path is not UTF-8"))?;
    let xml = |value: &str| -> Result<String, DeploymentError> {
        if value.chars().any(char::is_control) {
            return Err(DeploymentError::new(
                "invalid",
                "launchd path contains control characters",
            ));
        }
        Ok(value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;"))
    };
    Ok(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{}</string><string>run</string><string>--config</string><string>{}</string></array></dict></plist>\n", xml(label)?, xml(executable)?, xml(config)?).into_bytes())
}

/// Construct Eggup's launchd manager from explicit caller-selected domain,
/// target, plist path, and bootstrap policy.
#[allow(clippy::too_many_arguments)]
pub fn launchd_manager(
    domain: eggup_service::LaunchdDomain,
    target: &str,
    plist_path: &Path,
    executable: &Path,
    config_path: &Path,
    bootstrap_on_install: bool,
    transition_timeout: Duration,
) -> Result<eggup_service::LaunchdManager, DeploymentError> {
    match domain {
        eggup_service::LaunchdDomain::UserAgent => {
            let uid = target.strip_prefix("gui/").ok_or_else(|| {
                DeploymentError::new("invalid", "user launchd target must be gui/<uid>")
            })?;
            if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(DeploymentError::new(
                    "invalid",
                    "user launchd target must use a numeric uid",
                ));
            }
        }
        eggup_service::LaunchdDomain::SystemDaemon if target != "system" => {
            return Err(DeploymentError::new(
                "invalid",
                "system launchd target must be system",
            ));
        }
        eggup_service::LaunchdDomain::SystemDaemon => {}
    }
    let definition = render_launchd_plist(SERVICE_ID, executable, config_path)?;
    let install = eggup_service::LaunchdInstall::new(
        SERVICE_ID.to_owned(),
        domain,
        target.to_owned(),
        plist_path.to_path_buf(),
        definition,
        bootstrap_on_install,
        transition_timeout,
    )?;
    Ok(eggup_service::LaunchdManager::new(
        eggup_service::SystemExecutor::new(),
        install,
    ))
}

/// Construct the fixed Eggwork SCM product policy through Eggup's typed
/// Windows adapter. No custom account, dependencies, or elevation are used.
///
/// This is the recorded SCM *policy*, retained for review and for a future
/// service-host milestone. The operator surface does not expose it: hosted M004
/// qualification proved the installed daemon cannot answer an SCM start
/// request, so the mutating path fails closed before reaching this function.
#[cfg(windows)]
#[allow(dead_code)]
pub fn windows_scm_manager(
    start_type: eggup_service::WindowsStartType,
    transition_timeout: Duration,
) -> Result<eggup_service::WindowsScmManager, DeploymentError> {
    let install = eggup_service::WindowsScmInstall::new(
        SERVICE_ID,
        "Eggwork Node Daemon",
        start_type,
        eggup_service::WindowsErrorControl::Normal,
        None,
    )?
    .with_transition_timeout(transition_timeout)?;
    Ok(eggup_service::WindowsScmManager::new(install))
}

/// Candidate managers for the current host (selection policy owned by Eggup).
pub fn candidate_managers_for(
    facts: &eggup_service::HostFacts,
) -> Vec<eggup_service::CandidateManager> {
    eggup_service::candidate_managers(facts)
}

/// Deployment status snapshot for machine-readable operator output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentStatus {
    /// Pinned Eggup dependency versions qualified by this milestone.
    pub eggup_core_version: String,
    /// Pinned Eggup service dependency version.
    pub eggup_service_version: String,
    /// Install-unit artifact matrix.
    pub install_unit: Vec<InstallMember>,
    /// Helper compatibility outcome.
    pub helper: HelperCompatibility,
    /// Whether producer packaging remains outside this milestone.
    pub producer_packaging_external: bool,
}

impl DeploymentStatus {
    /// Build the status snapshot from local operator inputs.
    pub fn collect(helper_path: Option<&Path>, helper_required: bool) -> Self {
        Self {
            eggup_core_version: EGGUP_CORE_VERSION.into(),
            eggup_service_version: EGGUP_SERVICE_VERSION.into(),
            install_unit: install_unit_matrix(helper_required),
            helper: check_helper_compatibility(helper_path, env!("CARGO_PKG_VERSION")),
            producer_packaging_external: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggup_service::ServiceManager;
    use std::fs;
    use tempfile::TempDir;

    fn write_source(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn candidate(dir: &TempDir, member: &str, dest: &str, bytes: &[u8]) -> CandidateSource {
        let source = write_source(dir, &format!("src-{member}"), bytes);
        CandidateSource {
            member: member.into(),
            source,
            destination: dest.into(),
        }
    }

    fn install_root() -> TempDir {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("bin")).unwrap();
        root
    }

    #[test]
    fn install_unit_moves_daemon_and_helper_together() {
        let matrix = install_unit_matrix(true);
        assert_eq!(matrix.len(), 2);
        assert!(
            matrix
                .iter()
                .any(|m| m.member == MEMBER_DAEMON && m.required)
        );
        assert!(
            matrix
                .iter()
                .any(|m| m.member == MEMBER_HELPER && m.required)
        );
        let optional = install_unit_matrix(false);
        let helper = optional.iter().find(|m| m.member == MEMBER_HELPER).unwrap();
        assert!(!helper.required);
    }

    #[test]
    fn service_spec_distinguishes_executable_and_config() {
        let exe = Path::new("/opt/eggwork/bin/eggworkd");
        let config = Path::new("/etc/eggwork/node.json");
        let owned = service_spec_for_install(exe, Some(config)).unwrap();
        assert_eq!(owned.id().as_str(), SERVICE_ID);
        assert_eq!(owned.executable(), exe);
        assert_eq!(owned.config(), Some(config));
        assert!(owned.args().contains(&"run".to_owned()));

        let other_exe =
            service_spec_for_install(Path::new("/tmp/evil/eggworkd"), Some(config)).unwrap();
        let snapshot = eggup_service::RegistrationSnapshot {
            present: true,
            executable: Some(other_exe.executable().to_path_buf()),
            args: other_exe.args().to_vec(),
            config: other_exe.config().map(Path::to_path_buf),
            malformed: false,
        };
        assert_eq!(
            snapshot.ownership(&owned),
            eggup_service::Ownership::Foreign
        );
        assert!(service_spec_for_install(Path::new("relative/eggworkd"), Some(config)).is_err());
        assert!(service_spec_for_install(exe, Some(Path::new("relative.json"))).is_err());
    }

    #[test]
    fn owned_install_start_stop_restart_are_idempotent() {
        let mut manager = eggup_service::TestDoubleManager::new();
        let spec = service_spec_for_install(Path::new("/opt/eggwork/bin/eggworkd"), None).unwrap();
        manager.install(&spec).unwrap();
        for _ in 0..2 {
            assert!(
                manager
                    .start(&spec, Duration::from_secs(5))
                    .unwrap()
                    .completed()
            );
            assert!(
                manager
                    .stop(&spec, Duration::from_secs(5))
                    .unwrap()
                    .completed()
            );
        }
        assert!(
            manager
                .restart(&spec, Duration::from_secs(5))
                .unwrap()
                .completed()
        );
        let snapshot = manager.inspect(&spec).unwrap();
        assert_eq!(snapshot.ownership, eggup_service::Ownership::Owned);
        assert!(manager.uninstall(&spec).unwrap().completed());
    }

    #[test]
    fn foreign_same_name_registration_fails_closed() {
        let mut manager = eggup_service::TestDoubleManager::new();
        let wanted = service_spec_for_install(
            Path::new("/opt/eggwork/bin/eggworkd"),
            Some(Path::new("/etc/eggwork/node.json")),
        )
        .unwrap();
        let foreign = service_spec_for_install(
            Path::new("/opt/other/bin/eggworkd"),
            Some(Path::new("/etc/eggwork/node.json")),
        )
        .unwrap();
        manager.install(&foreign).unwrap();
        let snapshot = manager.inspect(&wanted).unwrap();
        assert_eq!(snapshot.ownership, eggup_service::Ownership::Foreign);
        assert!(manager.stop(&wanted, Duration::from_secs(5)).is_err());
        assert!(manager.start(&wanted, Duration::from_secs(5)).is_err());
        assert!(manager.restart(&wanted, Duration::from_secs(5)).is_err());
        assert!(manager.uninstall(&wanted).is_err());
        assert!(manager.install(&wanted).is_err());
    }

    #[test]
    fn unknown_malformed_registration_fails_closed() {
        let mut manager = eggup_service::TestDoubleManager::new();
        let id = eggup_service::ServiceId::new(SERVICE_ID).unwrap();
        manager.put_malformed(id);
        let spec = service_spec_for_install(Path::new("/opt/eggwork/bin/eggworkd"), None).unwrap();
        let snapshot = manager.inspect(&spec).unwrap();
        assert_eq!(snapshot.ownership, eggup_service::Ownership::Unknown);
        assert!(manager.stop(&spec, Duration::from_secs(5)).is_err());
        assert!(manager.uninstall(&spec).is_err());
    }

    #[test]
    fn candidate_validation_failure_happens_before_any_stop() {
        let mut manager = eggup_service::TestDoubleManager::new();
        let spec = service_spec_for_install(Path::new("/opt/eggwork/bin/eggworkd"), None).unwrap();
        manager.install(&spec).unwrap();
        manager.start(&spec, Duration::from_secs(5)).unwrap();
        let root = install_root();
        let bad = CandidateSource {
            member: "unknown-member".into(),
            source: PathBuf::from("/tmp/candidate"),
            destination: DEST_DAEMON.into(),
        };
        let policy = UpdatePolicy::default();
        let result = orchestrate_update(
            &mut manager,
            UpdateRequest {
                spec: &spec,
                installation_root: root.path(),
                release: "2026.09.27",
                sources: &[bad],
                drain_marker: None,
                policy,
            },
            &eggup_core::ExistingAsOwnedVerifier,
            || 0,
            || Ok(()),
        );
        assert!(result.is_err());
        let snapshot = manager.inspect(&spec).unwrap();
        assert!(snapshot.was_running);
    }

    #[test]
    fn drain_with_active_execution_blocks_update_without_force() {
        let mut manager = eggup_service::TestDoubleManager::new();
        let spec = service_spec_for_install(Path::new("/opt/eggwork/bin/eggworkd"), None).unwrap();
        manager.install(&spec).unwrap();
        let root = install_root();
        let inputs = TempDir::new().unwrap();
        let sources = vec![candidate(
            &inputs,
            MEMBER_DAEMON,
            DEST_DAEMON,
            b"daemon-bytes",
        )];
        let policy = UpdatePolicy {
            drain_timeout: Duration::from_millis(20),
            ..UpdatePolicy::default()
        };
        let result = orchestrate_update(
            &mut manager,
            UpdateRequest {
                spec: &spec,
                installation_root: root.path(),
                release: "2026.09.27",
                sources: &sources,
                drain_marker: None,
                policy,
            },
            &eggup_core::ExistingAsOwnedVerifier,
            || 2,
            || Ok(()),
        );
        assert!(result.is_err());
        assert!(!root.path().join(DEST_DAEMON).exists());

        let forced = UpdatePolicy {
            force: true,
            ..UpdatePolicy::default()
        };
        orchestrate_update(
            &mut manager,
            UpdateRequest {
                spec: &spec,
                installation_root: root.path(),
                release: "2026.09.27",
                sources: &sources,
                drain_marker: None,
                policy: forced,
            },
            &eggup_core::ExistingAsOwnedVerifier,
            || 2,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            fs::read(root.path().join(DEST_DAEMON)).unwrap(),
            b"daemon-bytes"
        );
    }

    #[test]
    fn update_after_drain_replaces_coherent_daemon_helper_pair() {
        let mut manager = eggup_service::TestDoubleManager::new();
        let spec = service_spec_for_install(Path::new("/opt/eggwork/bin/eggworkd"), None).unwrap();
        manager.install(&spec).unwrap();
        manager.start(&spec, Duration::from_secs(5)).unwrap();
        let root = install_root();
        let inputs = TempDir::new().unwrap();
        let sources = vec![
            candidate(&inputs, MEMBER_DAEMON, DEST_DAEMON, b"daemon-v2"),
            candidate(&inputs, MEMBER_HELPER, DEST_HELPER, b"helper-v2"),
        ];
        let receipt = orchestrate_update(
            &mut manager,
            UpdateRequest {
                spec: &spec,
                installation_root: root.path(),
                release: "2026.09.27",
                sources: &sources,
                drain_marker: None,
                policy: UpdatePolicy::default(),
            },
            &eggup_core::ExistingAsOwnedVerifier,
            || 0,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            receipt.disposition(),
            eggup_core::TransactionDisposition::Committed
        );
        assert_eq!(
            fs::read(root.path().join(DEST_DAEMON)).unwrap(),
            b"daemon-v2"
        );
        assert_eq!(
            fs::read(root.path().join(DEST_HELPER)).unwrap(),
            b"helper-v2"
        );
        assert!(manager.inspect(&spec).unwrap().was_running);
    }

    #[test]
    fn update_sets_persistent_drain_and_preserves_recovery_state() {
        let mut manager = eggup_service::TestDoubleManager::new();
        let spec = service_spec_for_install(Path::new("/opt/eggwork/bin/eggworkd"), None).unwrap();
        manager.install(&spec).unwrap();
        manager.start(&spec, Duration::from_secs(5)).unwrap();
        let root = install_root();
        // Recovery state lives outside the install unit and must survive.
        let recovery = root.path().join("state-executions.sqlite-sentinel");
        fs::write(&recovery, b"terminal-records").unwrap();
        let drain_dir = TempDir::new().unwrap();
        let drain_marker = drain_dir.path().join("node.drain");
        let inputs = TempDir::new().unwrap();
        let sources = vec![candidate(&inputs, MEMBER_DAEMON, DEST_DAEMON, b"daemon-v3")];
        let receipt = orchestrate_update(
            &mut manager,
            UpdateRequest {
                spec: &spec,
                installation_root: root.path(),
                release: "2026.09.27",
                sources: &sources,
                drain_marker: Some(&drain_marker),
                policy: UpdatePolicy::default(),
            },
            &eggup_core::ExistingAsOwnedVerifier,
            || 0,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            receipt.disposition(),
            eggup_core::TransactionDisposition::Committed
        );
        // Update entered persistent drain before replacement.
        assert!(crate::operations::is_persistently_draining(&drain_marker));
        // Recovery state is byte-identical; no execution completion fabricated.
        assert_eq!(fs::read(&recovery).unwrap(), b"terminal-records");
        assert_eq!(
            fs::read(root.path().join(DEST_DAEMON)).unwrap(),
            b"daemon-v3"
        );
        assert!(manager.inspect(&spec).unwrap().was_running);
    }

    #[test]
    fn replacement_failure_rolls_back_to_prior_generation() {
        let root = install_root();
        let inputs = TempDir::new().unwrap();
        fs::write(root.path().join("bin/eggworkd"), b"prior-daemon").unwrap();
        let before = eggup_core::hash_file(&root.path().join("bin/eggworkd")).unwrap();
        let verifier = eggup_core::ExactDigestVerifier::new(vec![(
            eggup_core::MemberId::new(MEMBER_DAEMON).unwrap(),
            before,
        )]);
        let tampered_source = write_source(&inputs, "tampered", b"staged-bytes");
        let member = eggup_core::ArtifactMember::new(
            eggup_core::MemberId::new(MEMBER_DAEMON).unwrap(),
            tampered_source.clone(),
            PathBuf::from(DEST_DAEMON),
        )
        .unwrap()
        .with_integrity(eggup_core::IntegrityRequirement::Sha256([9u8; 32]));
        let plan = eggup_core::InstallPlan::new(
            eggup_core::ProductId::new(PRODUCT_ID).unwrap(),
            eggup_core::ReleaseId::new("bad").unwrap(),
            root.path(),
            eggup_core::ArtifactSet::single(member).unwrap(),
        )
        .unwrap();
        let result =
            commit_without_health_check(plan, &verifier, eggup_core::AbsentPolicy::AllowCreate);
        assert!(result.is_err());
        assert_eq!(
            fs::read(root.path().join("bin/eggworkd")).unwrap(),
            b"prior-daemon"
        );
    }

    #[test]
    fn post_start_health_failure_rolls_back_while_backups_are_retained() {
        let root = install_root();
        let inputs = TempDir::new().unwrap();
        fs::write(root.path().join("bin/eggworkd"), b"prior-daemon").unwrap();
        let sources = vec![candidate(
            &inputs,
            MEMBER_DAEMON,
            DEST_DAEMON,
            b"new-daemon",
        )];
        let plan = build_install_plan(root.path(), "2026.09.27", &sources).unwrap();
        let receipt = commit_with_health_check(
            plan,
            &eggup_core::ExistingAsOwnedVerifier,
            eggup_core::AbsentPolicy::AllowCreate,
            eggup_core::PostCommitFailurePolicy::RollBack,
            || Err("bounded health probe failed".to_owned()),
        )
        .unwrap();
        assert!(receipt.rollback_performed());
        assert!(receipt.rollback_verified());
        assert_eq!(
            fs::read(root.path().join("bin/eggworkd")).unwrap(),
            b"prior-daemon"
        );

        let sources = vec![candidate(
            &inputs,
            MEMBER_DAEMON,
            DEST_DAEMON,
            b"new-daemon-keep",
        )];
        let plan = build_install_plan(root.path(), "2026.09.28", &sources).unwrap();
        let kept = commit_with_health_check(
            plan,
            &eggup_core::ExistingAsOwnedVerifier,
            eggup_core::AbsentPolicy::AllowCreate,
            eggup_core::PostCommitFailurePolicy::KeepInstalled,
            || Err("operator keeps installed".to_owned()),
        )
        .unwrap();
        assert!(!kept.rollback_performed());
        assert_eq!(
            fs::read(root.path().join("bin/eggworkd")).unwrap(),
            b"new-daemon-keep"
        );
    }

    #[test]
    fn installed_daemon_version_is_read_from_its_json_report() {
        // The post-install check was never exercised end-to-end before M004,
        // and a raw string comparison silently fails every real update because
        // the daemon answers `version` with a JSON object, not a bare string.
        assert_eq!(
            check_installed_daemon_version(b"{\"version\":\"0.1.1\"}", "0.1.1"),
            InstalledVersionCheck::Matched {
                version: "0.1.1".into()
            }
        );
        assert_eq!(
            check_installed_daemon_version(b"  {\"version\":\"0.1.1\"}\n", "0.1.1"),
            InstalledVersionCheck::Matched {
                version: "0.1.1".into()
            }
        );
        assert_eq!(
            check_installed_daemon_version(b"{\"version\":\"0.1.0\"}", "0.1.1"),
            InstalledVersionCheck::Mismatch {
                reported: "0.1.0".into()
            }
        );
        for unreadable in [
            &b""[..],
            &b"0.1.1"[..],
            &b"{\"schema_version\":1}"[..],
            &b"{\"version\":7}"[..],
            &b"[]"[..],
        ] {
            assert!(
                matches!(
                    check_installed_daemon_version(unreadable, "0.1.1"),
                    InstalledVersionCheck::Unreadable { .. }
                ),
                "{unreadable:?} must not be reported as a version"
            );
        }
        let oversized = vec![b'{'; MAX_VERSION_PROBE_BYTES + 1];
        assert!(matches!(
            check_installed_daemon_version(&oversized, "0.1.1"),
            InstalledVersionCheck::Unreadable { .. }
        ));
    }

    #[test]
    fn helper_version_skew_is_detected() {
        assert_eq!(
            check_helper_compatibility(None, env!("CARGO_PKG_VERSION")),
            HelperCompatibility::NotConfigured
        );
        assert_eq!(
            check_helper_compatibility(
                Some(Path::new("/nonexistent/eggwork-sandbox-helper")),
                env!("CARGO_PKG_VERSION")
            ),
            HelperCompatibility::Missing
        );
        let dir = TempDir::new().unwrap();
        let wrong = dir.path().join("eggwork-sandbox-helper");
        fs::write(&wrong, b"not-an-executable").unwrap();
        match check_helper_compatibility(Some(&wrong), env!("CARGO_PKG_VERSION")) {
            HelperCompatibility::Untrusted { .. } => {}
            other => panic!("expected untrusted, got {other:?}"),
        }
    }

    #[test]
    fn linux_host_reports_manager_facts_without_mutation() {
        let facts = platform_support().unwrap();
        assert_eq!(facts.os, std::env::consts::OS);
        let _ = candidate_managers_for(&facts);
    }

    #[test]
    fn systemd_unit_rendering_is_bounded_and_absolute_only() {
        let unit = render_systemd_unit(
            Path::new("/opt/eggwork/bin/eggworkd"),
            Path::new("/etc/eggwork/node.json"),
        )
        .unwrap();
        let text = String::from_utf8(unit).unwrap();
        assert!(
            text.contains(
                "ExecStart=/opt/eggwork/bin/eggworkd run --config /etc/eggwork/node.json"
            )
        );
        assert!(
            render_systemd_unit(Path::new("relative"), Path::new("/etc/eggwork/node.json"))
                .is_err()
        );
        assert!(
            render_systemd_unit(
                Path::new("/opt/Egg Work/bin/eggworkd"),
                Path::new("/etc/eggwork/node.json")
            )
            .is_err()
        );
        assert!(
            render_systemd_unit(
                Path::new("/opt/eggwork/bin/$eggworkd"),
                Path::new("/etc/eggwork/node.json")
            )
            .is_err()
        );
    }

    #[test]
    fn launchd_policy_uses_literal_argv_and_xml_escapes_paths() {
        let plist = render_launchd_plist(
            SERVICE_ID,
            Path::new("/opt/Egg&Work/eggworkd"),
            Path::new("/etc/Egg<Work/node.json"),
        )
        .unwrap();
        let text = String::from_utf8(plist).unwrap();
        assert!(text.contains("<string>/opt/Egg&amp;Work/eggworkd</string>"));
        assert!(text.contains("<string>/etc/Egg&lt;Work/node.json</string>"));
        assert!(text.contains("<string>run</string><string>--config</string>"));
        assert!(!text.contains("<key>KeepAlive</key>"));
        assert!(
            render_launchd_plist(
                SERVICE_ID,
                Path::new("/opt/eggwork/daemon\nstart"),
                Path::new("/etc/eggwork/node.json"),
            )
            .is_err()
        );
        assert!(
            launchd_manager(
                eggup_service::LaunchdDomain::UserAgent,
                "gui/not-a-uid",
                Path::new("/tmp/eggwork-node.plist"),
                Path::new("/opt/eggwork/eggworkd"),
                Path::new("/etc/eggwork/node.json"),
                false,
                Duration::from_secs(30),
            )
            .is_err()
        );
        assert!(
            launchd_manager(
                eggup_service::LaunchdDomain::UserAgent,
                "gui/501",
                Path::new("/tmp/eggwork-node.plist"),
                Path::new("/opt/eggwork/eggworkd"),
                Path::new("/etc/eggwork/node.json"),
                false,
                Duration::from_secs(30),
            )
            .is_ok()
        );
    }

    #[test]
    fn no_direct_service_manager_invocation_exists() {
        // Guard: Eggwork must reach managers only through eggup-service
        // adapters. Documentation may name the managers in backticks, but
        // non-test code must never contain them as command string literals.
        for file in [
            "src/deployment.rs",
            "src/operations.rs",
            "src/bin/eggworkd.rs",
        ] {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            let code = text
                .split("fn no_direct_service_manager_invocation_exists")
                .next()
                .unwrap_or_default();
            for forbidden in [
                "\"systemctl\"",
                "\"launchctl\"",
                "\"sc.exe\"",
                "\"crontab\"",
            ] {
                assert!(
                    !code.contains(forbidden),
                    "{file} must not invoke {forbidden} directly; use eggup-service adapters"
                );
            }
        }
        let adapter_boundary = std::fs::read_to_string("src/deployment.rs").unwrap_or_default();
        assert!(adapter_boundary.contains("eggup_service::ServiceManager"));
        assert!(adapter_boundary.contains("eggup_service::SystemdManager"));
    }

    #[test]
    fn only_compatible_helper_admits_required_isolation() {
        assert!(helper_satisfies_required_isolation(
            &HelperCompatibility::Compatible {
                version: env!("CARGO_PKG_VERSION").into(),
            }
        ));
        for denied in [
            HelperCompatibility::NotConfigured,
            HelperCompatibility::Missing,
            HelperCompatibility::Untrusted {
                reason: "probe failed".into(),
            },
            HelperCompatibility::VersionSkew {
                expected: env!("CARGO_PKG_VERSION").into(),
                found: "0.0.0".into(),
            },
        ] {
            assert!(
                !helper_satisfies_required_isolation(&denied),
                "{denied:?} must fail closed for required isolation, never downgrade"
            );
        }
    }

    #[test]
    fn rollback_restores_coherent_daemon_helper_pair() {
        let root = install_root();
        let inputs = TempDir::new().unwrap();
        fs::write(root.path().join("bin/eggworkd"), b"prior-daemon").unwrap();
        fs::write(
            root.path().join("bin/eggwork-sandbox-helper"),
            b"prior-helper",
        )
        .unwrap();
        let sources = vec![
            candidate(&inputs, MEMBER_DAEMON, DEST_DAEMON, b"new-daemon"),
            candidate(&inputs, MEMBER_HELPER, DEST_HELPER, b"new-helper"),
        ];
        let plan = build_install_plan(root.path(), "2026.09.27", &sources).unwrap();
        let receipt = commit_with_health_check(
            plan,
            &eggup_core::ExistingAsOwnedVerifier,
            eggup_core::AbsentPolicy::AllowCreate,
            eggup_core::PostCommitFailurePolicy::RollBack,
            || Err("bounded post-start health probe failed".to_owned()),
        )
        .unwrap();
        assert!(receipt.rollback_performed());
        assert!(receipt.rollback_verified());
        // A coherent pair is restored: neither member is left at the new
        // generation while the other rolls back.
        assert_eq!(
            fs::read(root.path().join(DEST_DAEMON)).unwrap(),
            b"prior-daemon"
        );
        assert_eq!(
            fs::read(root.path().join(DEST_HELPER)).unwrap(),
            b"prior-helper"
        );
    }

    #[test]
    fn deployment_surfaces_contain_no_secret_material() {
        let sentinel = "M004-SENTINEL-9f3c-helper-secret";
        let dir = TempDir::new().unwrap();
        let helper = dir.path().join(sentinel);
        fs::write(&helper, b"bytes").unwrap();
        let status = DeploymentStatus::collect(Some(&helper), true);
        let encoded = serde_json::to_string(&status).unwrap();
        assert!(
            !encoded.contains(sentinel),
            "deployment status must not embed helper paths"
        );
        let error = CandidateSource {
            member: MEMBER_DAEMON.into(),
            source: PathBuf::from(format!("/tmp/{sentinel}/candidate")),
            destination: DEST_DAEMON.into(),
        }
        .validate()
        .unwrap_err();
        assert!(!error.to_string().contains(sentinel));
        assert!(error.to_string().len() <= 256 + 32);
        let hostile = DeploymentError::new("candidate", "x".repeat(10_000));
        assert!(hostile.to_string().len() <= 256 + 32);
    }
}
