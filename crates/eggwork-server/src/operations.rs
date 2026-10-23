//! Local operator configuration and bounded inspection/maintenance helpers.

use crate::{
    Authorizer, FingerprintPrincipalResolver, NodeConfig, NodeGcReport, NodePrincipal, NodeServer,
    Operation, artifact::ArtifactStore, blob::BlobStore, telemetry::HostTelemetryConfig,
    workspace::WorkspaceManager,
};
use eggserve_core::tls::TlsServerConfig;
use eggwork_core::{ExecutionGeneration, ExecutionId, NodeId, PrincipalId};
use rusqlite::OptionalExtension;
use rustls::pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_GC_BATCH: usize = 1024;
const MAX_EXECUTION_PAGE: usize = 200;
const METRIC_NAMES: &[&str] = &[
    "executions_accepted",
    "rejected_route",
    "rejected_unauthenticated",
    "rejected_unauthorized",
    "rejected_invalid",
    "rejected_draining",
    "rejected_capability",
    "rejected_busy",
    "rejected_storage",
    "event_history_resync",
    "blob_upload_bytes",
    "blob_download_bytes",
    "stdout_bytes",
    "stderr_bytes",
    "terminal_succeeded",
    "terminal_failed",
    "terminal_cancelled",
    "terminal_timed_out",
    "terminal_interrupted",
    "cleanup_failures",
];

#[derive(Debug, Error)]
pub enum OperationsError {
    #[error("operator configuration is invalid")]
    Config,
    #[error("operator configuration could not be read")]
    Io(#[from] std::io::Error),
    #[error("operator data could not be read")]
    Data,
    #[error("operator request is outside its supported bounds")]
    Bounds,
    #[error("operator request names an unknown resource")]
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorConfig {
    pub schema_version: u16,
    pub node_id: String,
    pub bind: String,
    pub execution_root: PathBuf,
    pub database_path: PathBuf,
    pub blob_root: PathBuf,
    pub blob_quota_bytes: u64,
    pub workspace_root: PathBuf,
    pub workspace_quota_bytes: u64,
    pub max_active_executions: u32,
    pub lease_ttl_seconds: u64,
    pub sandbox_helper: Option<PathBuf>,
    pub tls: TlsFiles,
    pub clients: Vec<ClientGrant>,
    /// Optional host-telemetry provider.
    ///
    /// Absent means the node claims no host measurements at all: no provider
    /// is created, no task is started, and no telemetry is served.
    pub host_telemetry: Option<HostTelemetrySettings>,
}

/// Operator configuration for the optional host-telemetry provider.
///
/// The endpoint is a loopback address of a locally installed telemetry
/// daemon. It is validated when the configuration is validated, not when the
/// node starts, so a misconfigured endpoint is refused at load time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostTelemetrySettings {
    /// Loopback status endpoint of the local provider.
    pub endpoint: String,
    /// Seconds between provider polls.
    pub refresh_interval_seconds: u64,
    /// Seconds a cached observation stays fresh.
    pub freshness_seconds: u64,
}

impl HostTelemetrySettings {
    /// Validate the settings into a provider configuration.
    pub fn config(&self) -> Result<HostTelemetryConfig, OperationsError> {
        HostTelemetryConfig::new(
            &self.endpoint,
            self.refresh_interval_seconds,
            self.freshness_seconds,
        )
        .map_err(|_| OperationsError::Config)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsFiles {
    pub certificate_chain: PathBuf,
    pub private_key: PathBuf,
    pub client_ca: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientGrant {
    pub principal_id: String,
    /// Lowercase hex SHA-256 digest of the verified leaf certificate DER.
    pub certificate_sha256: String,
    pub operations: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub schema_version: u16,
    pub ready: bool,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// Whether the configuration-and-TLS check passed.
    ///
    /// `status` needs this to fail closed on an invalid configuration, and the
    /// alternative was calling `config.validate()` a second time. That is not
    /// free: `validate` calls `tls_config()`, which re-reads and re-parses the
    /// private key and rebuilds a `TlsServerConfig`.
    pub fn configuration_ok(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.name == "configuration" && check.ok)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageSummary {
    pub execution_bytes: u64,
    pub blob_bytes: u64,
    pub workspace_bytes: u64,
    pub artifact_database_bytes: u64,
    pub blob_quota_bytes: u64,
    pub workspace_quota_bytes: u64,
    pub filesystem_free_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OperatorMetrics {
    pub counters: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionPage {
    pub limit: usize,
    pub offset: usize,
    pub total: usize,
    pub active_executions: usize,
    pub states: BTreeMap<&'static str, usize>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub cleanup_failures: usize,
    pub next_offset: Option<usize>,
    pub executions: Vec<eggwork_core::ExecutionSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceInspection {
    pub kind: &'static str,
    pub id: String,
    pub metadata: serde_json::Value,
}

impl OperatorConfig {
    pub fn load(path: &Path) -> Result<Self, OperationsError> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_CONFIG_BYTES
            || config_writable_by_others(&metadata)
        {
            return Err(OperationsError::Config);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(path)?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let mut config: Self =
            serde_json::from_slice(&bytes).map_err(|_| OperationsError::Config)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        config.resolve_paths(base);
        Ok(config)
    }

    fn resolve_paths(&mut self, base: &Path) {
        for path in [
            &mut self.execution_root,
            &mut self.database_path,
            &mut self.blob_root,
            &mut self.workspace_root,
            &mut self.tls.certificate_chain,
            &mut self.tls.private_key,
            &mut self.tls.client_ca,
        ] {
            if path.is_relative() {
                *path = base.join(&*path);
            }
        }
        if let Some(path) = &mut self.sandbox_helper
            && path.is_relative()
        {
            *path = base.join(&*path);
        }
    }

    pub fn validate(&self) -> Result<(), OperationsError> {
        if self.schema_version != 1
            || NodeId::new(self.node_id.clone()).is_err()
            || self.bind.parse::<SocketAddr>().is_err()
            || self.max_active_executions == 0
            || self.max_active_executions > 65_536
            || self.lease_ttl_seconds == 0
            || self.lease_ttl_seconds > 86_400
            || self.blob_quota_bytes == 0
            || self.workspace_quota_bytes == 0
            || self.clients.is_empty()
            || self.clients.len() > 1024
        {
            return Err(OperationsError::Config);
        }
        let mut fingerprints = HashSet::new();
        for grant in &self.clients {
            if PrincipalId::new(grant.principal_id.clone()).is_err()
                || grant.certificate_sha256.len() != 64
                || !grant
                    .certificate_sha256
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit())
                || !fingerprints.insert(grant.certificate_sha256.to_ascii_lowercase())
                || grant.operations.is_empty()
                || grant
                    .operations
                    .iter()
                    .any(|name| operation(name).is_none())
            {
                return Err(OperationsError::Config);
            }
        }
        let _ = self.tls_config()?;
        if let Some(settings) = &self.host_telemetry {
            // A refused endpoint must fail the whole configuration: a node
            // that silently ignored a misconfigured provider would look like a
            // node with no telemetry at all, and the operator would have no
            // way to tell that from a node whose provider is down.
            settings.config()?;
        }
        for path in [&self.execution_root, &self.blob_root, &self.workspace_root] {
            let meta = fs::symlink_metadata(path).map_err(|_| OperationsError::Config)?;
            if !meta.is_dir() || meta.file_type().is_symlink() || !trusted_directory(&meta) {
                return Err(OperationsError::Config);
            }
        }
        // These roots must not overlap, and they must not contain one another.
        // `WorkspaceManager::reconcile` deletes every directory under
        // `workspace_root` that is not a known workspace storage key, so an
        // operator who pointed `workspace_root` at `blob_root` would have every
        // two-hex-character blob shard deleted on the next open — by the daemon
        // at startup and again by `eggworkd gc`. Reject the configuration
        // instead: a misconfiguration must not be able to destroy the blob store.
        let roots = [&self.execution_root, &self.blob_root, &self.workspace_root];
        for (index, path) in roots.iter().enumerate() {
            for other in &roots[index + 1..] {
                if path == other || path.starts_with(other) || other.starts_with(path) {
                    return Err(OperationsError::Config);
                }
            }
        }
        for path in [&self.database_path] {
            if let Some(parent) = path.parent()
                && (!parent.is_dir()
                    || fs::symlink_metadata(parent).is_ok_and(|meta| {
                        meta.file_type().is_symlink() || !trusted_directory(&meta)
                    }))
            {
                return Err(OperationsError::Config);
            }
        }
        if let Ok(metadata) = fs::symlink_metadata(&self.database_path)
            && (!metadata.is_file()
                || metadata.file_type().is_symlink()
                || !owned_writable_file(&metadata))
        {
            return Err(OperationsError::Config);
        }
        Ok(())
    }

    fn tls_config(&self) -> Result<TlsServerConfig, OperationsError> {
        let certs = read_certificates(&self.tls.certificate_chain)?;
        let roots = read_certificates(&self.tls.client_ca)?;
        let key_file = trusted_private_key(&self.tls.private_key)?;
        let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))
            .map_err(|_| OperationsError::Config)?
            .ok_or(OperationsError::Config)?;
        TlsServerConfig::builder()
            .single_identity(certs, key)
            .map_err(|_| OperationsError::Config)?
            .client_auth_required(roots)
            .map_err(|_| OperationsError::Config)?
            .build()
            .map_err(|_| OperationsError::Config)
    }

    pub fn redacted_json(&self) -> Result<String, OperationsError> {
        let mut value = serde_json::to_value(self).map_err(|_| OperationsError::Config)?;
        if let Some(tls) = value
            .get_mut("tls")
            .and_then(serde_json::Value::as_object_mut)
        {
            tls.insert("private_key".into(), "[REDACTED]".into());
        }
        if let Some(clients) = value
            .get_mut("clients")
            .and_then(serde_json::Value::as_array_mut)
        {
            for client in clients {
                if let Some(object) = client.as_object_mut() {
                    object.insert("certificate_sha256".into(), "[REDACTED]".into());
                }
            }
        }
        serde_json::to_string_pretty(&value).map_err(|_| OperationsError::Config)
    }

    pub fn node_config(&self) -> Result<NodeConfig, OperationsError> {
        Ok(NodeConfig {
            node_id: NodeId::new(self.node_id.clone()).map_err(|_| OperationsError::Config)?,
            bind: self.bind.parse().map_err(|_| OperationsError::Config)?,
            execution_root: self.execution_root.clone(),
            database_path: self.database_path.clone(),
            blob_root: self.blob_root.clone(),
            blob_quota_bytes: self.blob_quota_bytes,
            workspace_root: self.workspace_root.clone(),
            workspace_quota_bytes: self.workspace_quota_bytes,
            max_active_executions: self.max_active_executions,
            lease_ttl: Duration::from_secs(self.lease_ttl_seconds),
            tls: self.tls_config()?,
            host_telemetry: self
                .host_telemetry
                .as_ref()
                .map(HostTelemetrySettings::config)
                .transpose()?,
        })
    }

    pub fn principals(
        &self,
    ) -> Result<(FingerprintPrincipalResolver, GrantAuthorizer), OperationsError> {
        let mut identities = Vec::with_capacity(self.clients.len());
        let mut grants = HashMap::new();
        for client in &self.clients {
            let id = PrincipalId::new(client.principal_id.clone())
                .map_err(|_| OperationsError::Config)?;
            identities.push((client.certificate_sha256.to_ascii_lowercase(), id.clone()));
            let allowed = client
                .operations
                .iter()
                .filter_map(|name| operation(name))
                .collect();
            grants.insert(id, allowed);
        }
        Ok((
            FingerprintPrincipalResolver::new(identities),
            GrantAuthorizer(grants),
        ))
    }

    pub fn drain_path(&self) -> PathBuf {
        self.database_path.with_extension("drain")
    }
}

pub struct GrantAuthorizer(HashMap<PrincipalId, HashSet<Operation>>);

impl Authorizer for GrantAuthorizer {
    fn authorize(&self, principal: &NodePrincipal, operation: Operation) -> bool {
        self.0
            .get(&principal.id)
            .is_some_and(|allowed| allowed.contains(&operation))
    }
}

fn operation(name: &str) -> Option<Operation> {
    Some(match name {
        "capabilities" => Operation::Capabilities,
        "status" => Operation::Status,
        "telemetry" => Operation::Telemetry,
        "execute" => Operation::Execute,
        "observe" => Operation::Observe,
        "cancel" => Operation::Cancel,
        "renew" => Operation::Renew,
        "events" => Operation::Events,
        "blob-read" => Operation::BlobRead,
        "blob-write" => Operation::BlobWrite,
        "workspace-create" => Operation::WorkspaceCreate,
        "artifact-read" => Operation::ArtifactRead,
        _ => return None,
    })
}

fn read_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, OperationsError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || config_writable_by_others(&metadata)
    {
        return Err(OperationsError::Config);
    }
    let file = File::open(path)?;
    let certs = rustls_pemfile::certs(&mut BufReader::new(file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| OperationsError::Config)?;
    if certs.is_empty() {
        return Err(OperationsError::Config);
    }
    Ok(certs)
}

#[cfg(unix)]
fn trusted_directory(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    // Exactly `0700`, not "the owner has `rwx`". `mode & 0o700 == 0o700` also
    // accepts `0750` and `0755`, and this predicate gates `execution_root`,
    // `blob_root`, `workspace_root` and the database parent — the directories
    // holding every execution's stdout/stderr, every workspace and every blob.
    // A readable-by-others state directory silently exposes all of it while
    // `config validate` and `doctor` still report the configuration valid, and
    // the operator rule we publish (`docs/quickstart.md`) is owner-only `0700`.
    // The `0o022` clause is kept explicit: it is the property that actually
    // mattered, and stating it documents why `0o700` alone is insufficient.
    let mode = metadata.permissions().mode();
    metadata.uid() == rustix::process::geteuid().as_raw()
        && mode & 0o777 == 0o700
        && mode & 0o022 == 0
}

#[cfg(not(unix))]
fn trusted_directory(_: &fs::Metadata) -> bool {
    true
}

fn trusted_private_key(path: &Path) -> Result<File, OperationsError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || config_writable_by_others(&metadata)
        || !owned_writable_file(&metadata)
        || private_key_readable_by_others(&metadata)
    {
        return Err(OperationsError::Config);
    }
    Ok(File::open(path)?)
}

#[cfg(unix)]
fn config_writable_by_others(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    (!matches!(metadata.uid(), 0) && metadata.uid() != rustix::process::geteuid().as_raw())
        || metadata.permissions().mode() & 0o022 != 0
}

#[cfg(not(unix))]
fn config_writable_by_others(_: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn owned_writable_file(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.permissions().mode() & 0o200 != 0
}

#[cfg(not(unix))]
fn owned_writable_file(_: &fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn private_key_readable_by_others(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 != 0
}

#[cfg(not(unix))]
fn private_key_readable_by_others(_: &fs::Metadata) -> bool {
    false
}

pub async fn doctor(config: &OperatorConfig) -> DoctorReport {
    let mut checks = Vec::new();
    let config_ok = config.validate().is_ok();
    checks.push(DoctorCheck {
        name: "configuration",
        ok: config_ok,
        detail: if config_ok {
            "configuration and TLS policy are valid"
        } else {
            "configuration or TLS material is invalid"
        }
        .into(),
    });
    // The provider is *not* contacted here. `doctor` is an offline diagnostic:
    // a node whose telemetry provider happens to be down is still a healthy
    // node, and a check that failed on the provider's availability would
    // report the operator's provider outage as the node's fault. The endpoint
    // is not echoed either, so a diagnostic line cannot carry a provider
    // address into a log.
    checks.push(DoctorCheck {
        name: "host-telemetry",
        ok: true,
        detail: match config.host_telemetry.as_ref() {
            None => "host telemetry is disabled; the node reports no host measurements".into(),
            Some(settings) => format!(
                "host telemetry is enabled against a validated loopback provider endpoint (refresh {}s, freshness {}s)",
                settings.refresh_interval_seconds, settings.freshness_seconds
            ),
        },
    });
    // The helper is verified here and again inside `execution_capabilities`, which
    // calls `verify_trusted_helper` on the same path. That is deliberate: the
    // runner's answer is the one that gates admission, and caching a trust
    // verdict across the two calls would create exactly the window trust
    // verification exists to close. `doctor` is a one-shot operator diagnostic,
    // so the duplicated ancestor walk and file digest cost microseconds and are
    // not worth a cross-crate trust cache.
    let helper_ok = config
        .sandbox_helper
        .as_ref()
        .is_none_or(|helper| trusted_helper(helper));
    let capabilities = if let Some(helper) = config.sandbox_helper.as_ref().filter(|_| helper_ok) {
        let runner = eggwork_runner::LocalProcessRunner::new(
            eggwork_runner::TrustedLandlockSetup::new(helper),
        );
        runner.execution_capabilities().await
    } else {
        Vec::new()
    };
    checks.push(DoctorCheck {
        name: "sandbox-resource-backend",
        ok: helper_ok,
        detail: if config.sandbox_helper.is_none() {
            "no trusted sandbox helper configured; required isolation fails closed and resource limits are not advertised".into()
        } else if !helper_ok {
            "configured sandbox helper is missing or fails installation ownership checks".into()
        } else if capabilities.is_empty() {
            "trusted helper configured; no execution capability passed runtime probes".into()
        } else {
            format!("runtime-probed capabilities: {}", capabilities.join(", "))
        },
    });
    let bind: Result<SocketAddr, _> = config.bind.parse();
    let listener_ok = bind.as_ref().is_ok_and(|addr| {
        TcpListener::bind(addr).is_ok()
            || std::net::TcpStream::connect_timeout(addr, Duration::from_millis(150)).is_ok()
    });
    checks.push(DoctorCheck {
        name: "listener",
        ok: listener_ok,
        detail: if listener_ok {
            "bind address is available"
        } else {
            "bind address is invalid or unavailable"
        }
        .into(),
    });
    for (name, path) in [
        ("execution-root", &config.execution_root),
        ("blob-root", &config.blob_root),
        ("workspace-root", &config.workspace_root),
    ] {
        let ok = fs::metadata(path).is_ok_and(|m| m.is_dir()) && fs::read_dir(path).is_ok();
        checks.push(DoctorCheck {
            name,
            ok,
            detail: if ok {
                "directory is available"
            } else {
                "directory is missing or unreadable"
            }
            .into(),
        });
    }
    let free = fs2::available_space(&config.execution_root).ok();
    checks.push(DoctorCheck {
        name: "storage-headroom",
        ok: free.is_some_and(|bytes| bytes > 0),
        detail: free
            .map(|bytes| format!("{bytes} bytes available"))
            .unwrap_or_else(|| "available space could not be measured".into()),
    });
    DoctorReport {
        schema_version: 1,
        ready: checks.iter().all(|check| check.ok),
        checks,
    }
}

/// Whether the configured sandbox helper passes the runner's trust boundary.
///
/// This deliberately delegates instead of restating the policy. A stricter
/// local copy denies a helper the runner would accept, which turns every
/// unprivileged user-scope installation into a node that can never advertise
/// required filesystem isolation, and makes `doctor` contradict the enforcement
/// path it is meant to describe.
fn trusted_helper(path: &Path) -> bool {
    eggwork_runner::verify_trusted_helper(path).is_ok()
}

pub fn set_persistent_drain(path: &Path, draining: bool) -> Result<(), OperationsError> {
    if draining {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension(format!("drain.{}.tmp", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(b"draining\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
    } else {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

pub fn is_persistently_draining(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

pub async fn execution_page(
    config: &OperatorConfig,
    limit: usize,
    offset: usize,
) -> Result<ExecutionPage, OperationsError> {
    let limit = limit.clamp(1, MAX_EXECUTION_PAGE);
    let offset = offset.min(2048);
    require_regular_file(&config.database_path)?;
    let mut executions = load_snapshots(config)?;
    let total = executions.len();
    let active_executions = executions
        .iter()
        .filter(|snapshot| {
            matches!(
                snapshot.state,
                eggwork_core::ExecutionState::Accepted
                    | eggwork_core::ExecutionState::Preparing
                    | eggwork_core::ExecutionState::Running
                    | eggwork_core::ExecutionState::Cancelling
            )
        })
        .count();
    let mut states = BTreeMap::new();
    let mut stdout_bytes = 0u64;
    let mut stderr_bytes = 0u64;
    let mut cleanup_failures = 0usize;
    for snapshot in &executions {
        *states.entry(state_label(&snapshot.state)).or_insert(0) += 1;
        if let Some(result) = &snapshot.result {
            stdout_bytes = stdout_bytes.saturating_add(result.stdout_bytes);
            stderr_bytes = stderr_bytes.saturating_add(result.stderr_bytes);
            cleanup_failures += usize::from(result.cleanup_warning.is_some());
        }
    }
    executions.sort_by(|a, b| {
        (a.execution_id.as_str(), a.generation.get())
            .cmp(&(b.execution_id.as_str(), b.generation.get()))
    });
    executions.drain(..offset.min(executions.len()));
    executions.truncate(limit);
    let next_offset = (offset + executions.len() < total).then_some(offset + executions.len());
    Ok(ExecutionPage {
        limit,
        offset,
        total,
        active_executions,
        states,
        stdout_bytes,
        stderr_bytes,
        cleanup_failures,
        next_offset,
        executions,
    })
}

/// Return the durable active-execution count for update quiescence. Any
/// storage or decoding error fails closed as a nonzero count.
pub fn active_execution_count(config: &OperatorConfig) -> usize {
    match fs::symlink_metadata(&config.database_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return 0,
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        _ => return usize::MAX,
    }
    match count_active_executions(config) {
        Ok(count) => count,
        Err(_) => usize::MAX,
    }
}

/// Count non-terminal executions in SQL rather than by decoding snapshots.
///
/// `wait_for_quiescence` polls `active_execution_count` on a timer for the whole
/// drain window. It used to go through `load_snapshots`, which opened a fresh
/// connection and `serde_json`-decoded up to 2048 durable snapshots on every
/// poll — thousands of connections and millions of decodes over one update —
/// only to filter four state values that `executions.state` already stores as a
/// real column. The literals below are the `store::state_name` spellings, which
/// are PascalCase, not the snake_case used by `state_label` for display.
fn count_active_executions(config: &OperatorConfig) -> Result<usize, OperationsError> {
    require_regular_file(&config.database_path)?;
    let db = read_only_database(&config.database_path)?;
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM executions
             WHERE state IN ('Accepted','Preparing','Running','Cancelling')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| OperationsError::Data)?;
    Ok(count.max(0) as usize)
}

pub async fn execution_show(
    config: &OperatorConfig,
    id: &str,
    generation: Option<u64>,
) -> Result<eggwork_core::ExecutionSnapshot, OperationsError> {
    let id = ExecutionId::new(id.to_owned()).map_err(|_| OperationsError::NotFound)?;
    let generation = generation
        .map(ExecutionGeneration::new)
        .transpose()
        .map_err(|_| OperationsError::NotFound)?;
    require_regular_file(&config.database_path)?;
    let db = read_only_database(&config.database_path)?;
    let bytes: Option<Vec<u8>> = match generation {
        Some(generation) => db
            .query_row(
                "SELECT snapshot_json FROM executions WHERE execution_id=?1 AND generation=?2",
                rusqlite::params![id.as_str(), generation.get() as i64],
                |row| row.get(0),
            )
            .optional(),
        None => db
            .query_row(
                "SELECT snapshot_json FROM executions WHERE execution_id=?1 ORDER BY generation DESC LIMIT 1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional(),
    }
    .map_err(|_| OperationsError::Data)?;
    serde_json::from_slice(&bytes.ok_or(OperationsError::NotFound)?)
        .map_err(|_| OperationsError::Data)
}

fn load_snapshots(
    config: &OperatorConfig,
) -> Result<Vec<eggwork_core::ExecutionSnapshot>, OperationsError> {
    match fs::symlink_metadata(&config.database_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(OperationsError::Data),
        Ok(_) => {}
    }
    require_regular_file(&config.database_path)?;
    let db = read_only_database(&config.database_path)?;
    let mut statement = db
        .prepare("SELECT snapshot_json FROM executions ORDER BY execution_id,generation LIMIT 2049")
        .map_err(|_| OperationsError::Data)?;
    let rows = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|_| OperationsError::Data)?;
    let snapshots = rows
        .map(|row| {
            let encoded = row.map_err(|_| OperationsError::Data)?;
            serde_json::from_slice(&encoded).map_err(|_| OperationsError::Data)
        })
        .collect::<Result<Vec<eggwork_core::ExecutionSnapshot>, _>>()?;
    if snapshots.len() > 2048 {
        return Err(OperationsError::Bounds);
    }
    Ok(snapshots)
}

fn state_label(state: &eggwork_core::ExecutionState) -> &'static str {
    use eggwork_core::ExecutionState::*;
    match state {
        Accepted => "accepted",
        Preparing => "preparing",
        Running => "running",
        Cancelling => "cancelling",
        Succeeded => "succeeded",
        Failed => "failed",
        Cancelled => "cancelled",
        TimedOut => "timed_out",
        Interrupted => "interrupted",
    }
}

fn tree_bytes(path: &Path) -> Result<u64, OperationsError> {
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(current) = pending.pop() {
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            for entry in fs::read_dir(current)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(total)
}

fn require_regular_file(path: &Path) -> Result<(), OperationsError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| OperationsError::Data)?;
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(OperationsError::Data)
    }
}

fn file_family_bytes(path: &Path) -> u64 {
    [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
    .iter()
    .filter_map(|candidate| fs::symlink_metadata(candidate).ok())
    .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    .fold(0u64, |sum, metadata| sum.saturating_add(metadata.len()))
}

pub fn storage_summary(config: &OperatorConfig) -> Result<StorageSummary, OperationsError> {
    let artifact_db = crate::artifact::metadata_path(&config.database_path);
    Ok(StorageSummary {
        execution_bytes: file_family_bytes(&config.database_path),
        blob_bytes: tree_bytes(&config.blob_root)?,
        workspace_bytes: tree_bytes(&config.workspace_root)?,
        artifact_database_bytes: file_family_bytes(&artifact_db),
        blob_quota_bytes: config.blob_quota_bytes,
        workspace_quota_bytes: config.workspace_quota_bytes,
        filesystem_free_bytes: fs2::available_space(&config.execution_root).ok(),
    })
}

pub fn metrics_snapshot(config: &OperatorConfig) -> Result<OperatorMetrics, OperationsError> {
    let mut counters = METRIC_NAMES
        .iter()
        .map(|name| ((*name).to_owned(), 0))
        .collect::<BTreeMap<_, _>>();
    match fs::symlink_metadata(&config.database_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OperatorMetrics { counters });
        }
        Err(_) => return Err(OperationsError::Data),
        Ok(_) => {}
    }
    require_regular_file(&config.database_path)?;
    let db = read_only_database(&config.database_path)?;
    let mut statement = match db.prepare("SELECT name,value FROM node_metrics") {
        Ok(statement) => statement,
        Err(_) => return Ok(OperatorMetrics { counters }),
    };
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|_| OperationsError::Data)?;
    for row in rows {
        let (name, value) = row.map_err(|_| OperationsError::Data)?;
        if let Some(counter) = counters.get_mut(&name) {
            *counter = value.max(0) as u64;
        }
    }
    Ok(OperatorMetrics { counters })
}

pub async fn collect_garbage(
    config: &OperatorConfig,
    dry_run: bool,
    requested_limit: usize,
) -> Result<NodeGcReport, OperationsError> {
    let limit = requested_limit.clamp(1, MAX_GC_BATCH);
    if dry_run {
        return dry_run_gc(config, limit);
    }
    let _state_lock = acquire_maintenance_lock(&config.database_path)?;
    let workspaces = WorkspaceManager::open(&config.workspace_root, config.workspace_quota_bytes)
        .map_err(|_| OperationsError::Data)?;
    let blobs = BlobStore::open(&config.blob_root, config.blob_quota_bytes)
        .map_err(|_| OperationsError::Data)?;
    let artifacts = ArtifactStore::open(crate::artifact::metadata_path(&config.database_path))
        .map_err(|_| OperationsError::Data)?;
    let now = crate::artifact::now_unix_ms();
    let (workspace_candidates, workspace_logical_bytes, workspaces_deleted) = workspaces
        .garbage_collect(now, limit, dry_run)
        .map_err(|_| OperationsError::Data)?;
    let (manifest_candidates, manifests_deleted) = workspaces
        .manifest_garbage_collect(now, limit, dry_run, &blobs)
        .map_err(|_| OperationsError::Data)?;
    let artifact_report = artifacts
        .garbage_collect(now, limit, dry_run)
        .map_err(|_| OperationsError::Data)?;
    let blob_report = blobs
        .garbage_collect(now, limit, dry_run)
        .await
        .map_err(|_| OperationsError::Data)?;
    Ok(NodeGcReport {
        workspace_candidates,
        workspace_logical_bytes,
        workspaces_deleted,
        manifest_candidates,
        manifests_deleted,
        artifact_candidates: artifact_report.candidate_artifacts,
        artifacts_deleted: artifact_report.deleted_artifacts,
        expired_blob_references: blob_report.expired_references_removed,
        blob_candidates: blob_report.candidate_blobs,
        blob_candidate_bytes: blob_report.candidate_bytes,
        blobs_deleted: blob_report.removed_blobs,
        blob_bytes_deleted: blob_report.removed_bytes,
        dry_run,
    })
}

fn acquire_maintenance_lock(database_path: &Path) -> Result<File, OperationsError> {
    use fs2::FileExt;
    let lock_path = database_path.with_extension("lock");
    // Refuse a pre-existing path that is not an owner-only regular file before
    // adopting it. `create(true)` opens whatever is already there, and without
    // this check a local user who can write the state directory could pre-create
    // the lock as a symlink: the victim would then open the *target*, lock it,
    // and every competing `flock` — including the node's own startup lock, which
    // uses the same pattern — would fail forever with a generic error. This is
    // the same discipline `require_regular_file` and `trusted_private_key`
    // already apply to every other path we adopt.
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(OperationsError::Data);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if metadata.uid() != rustix::process::geteuid().as_raw()
                    || metadata.permissions().mode() & 0o077 != 0
                {
                    return Err(OperationsError::Data);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(OperationsError::Data),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(lock_path)?;
    file.try_lock_exclusive()
        .map_err(|_| OperationsError::Data)?;
    Ok(file)
}

fn read_only_database(path: &Path) -> Result<rusqlite::Connection, OperationsError> {
    rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| OperationsError::Data)
}

fn dry_run_gc(config: &OperatorConfig, limit: usize) -> Result<NodeGcReport, OperationsError> {
    // The preview owns no candidate predicate of its own. It asks each owner
    // module for the set the collector would act on, and the owner evaluates
    // the same statement the collector evaluates - so the preview and the
    // apply pass cannot answer different questions.
    //
    // The owner entry points open the store's metadata database read-only.
    // They do not open the store: `BlobStore::open`, `WorkspaceManager::open`
    // and `ArtifactStore::open` all create and reconcile, and a preview that
    // created the store it reports on would not be a preview.
    let now = crate::artifact::now_unix_ms();
    let workspaces = crate::workspace::preview_gc(
        &crate::workspace::metadata_path(&config.workspace_root),
        now,
        limit,
    )
    .map_err(|_| OperationsError::Data)?;
    let artifacts = crate::artifact::preview_gc(
        &crate::artifact::metadata_path(&config.database_path),
        now,
        limit,
    )
    .map_err(|_| OperationsError::Data)?;
    let blobs = crate::blob::preview_gc(&crate::blob::metadata_path(&config.blob_root), now, limit)
        .map_err(|_| OperationsError::Data)?;
    Ok(NodeGcReport {
        workspace_candidates: workspaces.workspaces.len() as u64,
        workspace_logical_bytes: workspaces.workspaces.iter().fold(0u64, |sum, candidate| {
            sum.saturating_add(candidate.logical_bytes)
        }),
        workspaces_deleted: 0,
        manifest_candidates: workspaces.manifests.len() as u64,
        manifests_deleted: 0,
        artifact_candidates: artifacts.artifacts.len() as u64,
        artifacts_deleted: 0,
        expired_blob_references: blobs.expired_references,
        blob_candidates: blobs.candidates.len() as u64,
        blob_candidate_bytes: blobs
            .candidates
            .iter()
            .fold(0u64, |sum, (_, size)| sum.saturating_add(*size)),
        blobs_deleted: 0,
        blob_bytes_deleted: 0,
        dry_run: true,
    })
}

pub fn inspect_blob(
    config: &OperatorConfig,
    digest: &str,
) -> Result<ResourceInspection, OperationsError> {
    let digest = eggwork_core::BlobDigest::parse(digest.to_owned())
        .map_err(|_| OperationsError::NotFound)?;
    // The shard directory and the object path come from the store's own path
    // ownership. The inspection verb runs against an offline store, so it
    // must not need a store instance - and a store instance would mean the
    // creating, reconciling open that `BlobStore::open` performs.
    let shard = crate::blob::shard_path(&config.blob_root, &digest);
    let shard_meta = fs::symlink_metadata(&shard).map_err(|_| OperationsError::NotFound)?;
    if !shard_meta.is_dir() || shard_meta.file_type().is_symlink() {
        return Err(OperationsError::NotFound);
    }
    let path = crate::blob::object_path(&config.blob_root, &digest);
    let metadata = fs::symlink_metadata(&path).map_err(|_| OperationsError::NotFound)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OperationsError::NotFound);
    }
    let db = read_only_database(&crate::blob::metadata_path(&config.blob_root))?;
    let size_recorded: Option<(i64, i64)> = db
        .query_row(
            "SELECT size_bytes,reference_count FROM blobs WHERE digest=?1",
            [digest.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| OperationsError::Data)?;
    let (size_recorded, references) = size_recorded.ok_or(OperationsError::NotFound)?;
    if size_recorded.max(0) as u64 != metadata.len() {
        return Err(OperationsError::Data);
    }
    Ok(ResourceInspection {
        kind: "blob",
        id: digest.as_str().into(),
        metadata: serde_json::json!({"size_bytes": metadata.len(), "reference_count": references.max(0), "stored": true}),
    })
}

pub fn inspect_workspace(
    config: &OperatorConfig,
    id: &str,
) -> Result<ResourceInspection, OperationsError> {
    let id =
        eggwork_core::WorkspaceId::new(id.to_owned()).map_err(|_| OperationsError::NotFound)?;
    let db = read_only_database(&crate::workspace::metadata_path(&config.workspace_root))?;
    let row: Option<(String, i64, String, Option<i64>)> = db.query_row("SELECT state, logical_bytes, manifest_digest, expires_unix_ms FROM workspaces WHERE workspace_id=?1", [id.as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_| OperationsError::Data)?;
    let (state, logical, digest, expires) = row.ok_or(OperationsError::NotFound)?;
    Ok(ResourceInspection {
        kind: "workspace",
        id: id.as_str().into(),
        metadata: serde_json::json!({"state": state, "logical_bytes": logical.max(0), "manifest_digest": digest, "expires_unix_ms": expires}),
    })
}

pub fn inspect_artifact(
    config: &OperatorConfig,
    id: &str,
) -> Result<ResourceInspection, OperationsError> {
    let id = eggwork_core::ArtifactId::new(id.to_owned()).map_err(|_| OperationsError::NotFound)?;
    let db = read_only_database(&crate::artifact::metadata_path(&config.database_path))?;
    let row: Option<Vec<u8>> = db
        .query_row(
            "SELECT record_json FROM artifacts WHERE artifact_id=?1",
            [id.as_str()],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| OperationsError::Data)?;
    let record: eggwork_core::ArtifactRecord =
        serde_json::from_slice(&row.ok_or(OperationsError::NotFound)?)
            .map_err(|_| OperationsError::Data)?;
    Ok(ResourceInspection {
        kind: "artifact",
        id: id.as_str().into(),
        metadata: serde_json::to_value(record).map_err(|_| OperationsError::Data)?,
    })
}

pub async fn start_server(config: &OperatorConfig) -> Result<NodeServer, OperationsError> {
    let node_config = config.node_config()?;
    let (resolver, authorizer) = config.principals()?;
    let runner: Arc<eggwork_runner::LocalProcessRunner> =
        if let Some(helper) = &config.sandbox_helper {
            Arc::new(eggwork_runner::LocalProcessRunner::new(
                eggwork_runner::TrustedLandlockSetup::new(helper),
            ))
        } else {
            Arc::new(eggwork_runner::LocalProcessRunner::new(
                eggwork_runner::NoExecutionSetup,
            ))
        };
    NodeServer::start(
        node_config,
        runner,
        Arc::new(resolver),
        Arc::new(authorizer),
    )
    .await
    .map_err(|_| OperationsError::Config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn config(root: &Path) -> OperatorConfig {
        OperatorConfig {
            schema_version: 1,
            node_id: "node-test".into(),
            bind: "127.0.0.1:0".into(),
            execution_root: root.join("execution"),
            database_path: root.join("state/executions.sqlite"),
            blob_root: root.join("blobs"),
            blob_quota_bytes: 4096,
            workspace_root: root.join("workspaces"),
            workspace_quota_bytes: 4096,
            max_active_executions: 2,
            lease_ttl_seconds: 60,
            sandbox_helper: None,
            tls: TlsFiles {
                certificate_chain: root.join("server.pem"),
                private_key: root.join("super-secret-key.pem"),
                client_ca: root.join("client-ca.pem"),
            },
            clients: vec![ClientGrant {
                principal_id: "controller".into(),
                certificate_sha256: "ab".repeat(32),
                operations: vec!["status".into()],
            }],
            host_telemetry: None,
        }
    }

    #[test]
    fn redacted_config_print_hides_private_key_path_and_client_fingerprints() {
        let temp = TempDir::new().unwrap();
        let encoded = config(temp.path()).redacted_json().unwrap();
        assert!(!encoded.contains("super-secret-key.pem"));
        assert!(!encoded.contains(&"ab".repeat(32)));
        assert!(encoded.contains("[REDACTED]"));
    }

    /// A configuration document in the shape the loader accepts.
    fn config_json(root: &Path) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "node_id": "node-test",
            "bind": "127.0.0.1:0",
            "execution_root": root.join("execution"),
            "database_path": root.join("state/executions.sqlite"),
            "blob_root": root.join("blobs"),
            "blob_quota_bytes": 4096,
            "workspace_root": root.join("workspaces"),
            "workspace_quota_bytes": 4096,
            "max_active_executions": 2,
            "lease_ttl_seconds": 60,
            "tls": {
                "certificate_chain": root.join("server.pem"),
                "private_key": root.join("server-key.pem"),
                "client_ca": root.join("client-ca.pem")
            },
            "clients": [{
                "principal_id": "controller",
                "certificate_sha256": "ab".repeat(32),
                "operations": ["status"]
            }]
        })
    }

    #[test]
    fn a_configuration_without_host_telemetry_asks_for_none() {
        let temp = TempDir::new().unwrap();
        let parsed: OperatorConfig =
            serde_json::from_value(config_json(temp.path())).expect("config");
        assert!(parsed.host_telemetry.is_none());
    }

    #[test]
    fn a_configuration_with_host_telemetry_is_read_and_validated() {
        let temp = TempDir::new().unwrap();
        let mut value = config_json(temp.path());
        value["host_telemetry"] = serde_json::json!({
            "endpoint": "http://127.0.0.1:18080/v2/status",
            "refresh_interval_seconds": 15,
            "freshness_seconds": 60
        });
        let parsed: OperatorConfig = serde_json::from_value(value).expect("config");
        let settings = parsed.host_telemetry.expect("settings");
        assert!(settings.config().is_ok());
    }

    #[test]
    fn a_misconfigured_host_telemetry_endpoint_is_refused() {
        for endpoint in [
            "http://example.com:18080/v2/status",
            "http://localhost:18080/v2/status",
            "https://127.0.0.1:18080/v2/status",
            "http://127.0.0.1:18080/",
            "http://127.0.0.1:18080/v2/status?probe=1",
        ] {
            let settings = HostTelemetrySettings {
                endpoint: endpoint.into(),
                refresh_interval_seconds: 15,
                freshness_seconds: 60,
            };
            assert!(
                settings.config().is_err(),
                "endpoint {endpoint} must be refused"
            );
        }
    }

    #[test]
    fn an_unknown_host_telemetry_field_is_refused() {
        let temp = TempDir::new().unwrap();
        let mut value = config_json(temp.path());
        value["host_telemetry"] = serde_json::json!({
            "endpoint": "http://127.0.0.1:18080/v2/status",
            "refresh_interval_seconds": 15,
            "freshness_seconds": 60,
            "unexpected": true
        });
        assert!(serde_json::from_value::<OperatorConfig>(value).is_err());
    }

    /// A configuration with real TLS material and owner-only directories.
    fn installed_config(root: &Path) -> OperatorConfig {
        use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
        let _ = rustls::crypto::ring::default_provider().install_default();
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(vec!["eggwork test CA".into()]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let server_key = KeyPair::generate().unwrap();
        let server_params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        let server = server_params.signed_by(&server_key, &ca, &ca_key).unwrap();
        fs::write(root.join("server.pem"), server.pem()).unwrap();
        fs::write(root.join("client-ca.pem"), ca.pem()).unwrap();
        let key_path = root.join("server-key.pem");
        fs::write(&key_path, server_key.serialize_pem()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The loader refuses group- or world-writable material, and the
            // process umask is not ours to rely on.
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::set_permissions(root.join("server.pem"), fs::Permissions::from_mode(0o644))
                .unwrap();
            fs::set_permissions(
                root.join("client-ca.pem"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        }
        for name in ["execution", "blobs", "workspaces", "state"] {
            let path = root.join(name);
            fs::create_dir_all(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let parsed: OperatorConfig = serde_json::from_value(config_json(root)).expect("config");
        parsed
    }

    #[tokio::test]
    async fn doctor_reports_that_host_telemetry_is_off_by_default() {
        let temp = TempDir::new().unwrap();
        let config = installed_config(temp.path());
        assert!(config.validate().is_ok());
        let report = doctor(&config).await;
        let check = report
            .checks
            .iter()
            .find(|check| check.name == "host-telemetry")
            .expect("host-telemetry check");
        assert!(check.ok);
        assert!(check.detail.contains("disabled"));
    }

    #[tokio::test]
    async fn doctor_reports_a_configured_host_telemetry_provider() {
        let temp = TempDir::new().unwrap();
        let mut config = installed_config(temp.path());
        config.host_telemetry = Some(HostTelemetrySettings {
            endpoint: "http://127.0.0.1:18080/v2/status".into(),
            refresh_interval_seconds: 15,
            freshness_seconds: 60,
        });
        assert!(config.validate().is_ok());
        let report = doctor(&config).await;
        let check = report
            .checks
            .iter()
            .find(|check| check.name == "host-telemetry")
            .expect("host-telemetry check");
        assert!(check.ok);
        assert!(check.detail.contains("enabled"));
        // A diagnostic line must not carry the provider's address.
        assert!(!check.detail.contains("127.0.0.1"));
    }

    #[test]
    fn drain_state_survives_process_restart_and_undrain_removes_marker() {
        let temp = TempDir::new().unwrap();
        let marker = temp.path().join("node.drain");
        set_persistent_drain(&marker, true).unwrap();
        // A newly started server uses the same marker as its initial state.
        assert!(is_persistently_draining(&marker));
        set_persistent_drain(&marker, false).unwrap();
        assert!(!is_persistently_draining(&marker));
    }

    #[test]
    fn tree_size_never_follows_symlinks() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::write(&outside, vec![0u8; 512]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        assert_eq!(tree_bytes(&root).unwrap(), 0);
    }

    #[tokio::test]
    async fn execution_listing_is_bounded_and_paginates_over_durable_records() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(&config.database_path).unwrap();
        db.execute_batch(
            "CREATE TABLE executions(execution_id TEXT,generation INTEGER,snapshot_json BLOB);",
        )
        .unwrap();
        for index in 0..3u64 {
            let id = ExecutionId::new(format!("exec-{index}")).unwrap();
            let snapshot = eggwork_core::ExecutionSnapshot {
                schema_version: 1,
                execution_id: id.clone(),
                generation: ExecutionGeneration::new(1).unwrap(),
                state: if index == 1 {
                    eggwork_core::ExecutionState::Running
                } else {
                    eggwork_core::ExecutionState::Succeeded
                },
                result: None,
            };
            db.execute(
                "INSERT INTO executions VALUES (?1,1,?2)",
                rusqlite::params![id.as_str(), serde_json::to_vec(&snapshot).unwrap()],
            )
            .unwrap();
        }
        drop(db);
        let first = execution_page(&config, 1, 0).await.unwrap();
        assert_eq!(first.total, 3);
        assert_eq!(first.active_executions, 1);
        assert_eq!(first.executions[0].execution_id.as_str(), "exec-0");
        assert_eq!(first.next_offset, Some(1));
        let next = execution_page(&config, 1, first.next_offset.unwrap())
            .await
            .unwrap();
        assert_eq!(next.executions[0].execution_id.as_str(), "exec-1");
        assert_eq!(next.next_offset, Some(2));
    }

    /// The three data-plane stores, built through their owner modules.
    ///
    /// The fixture this replaces hand-built a `blobs` table whose columns were
    /// `(reference_count, size_bytes, last_used_unix_ms)`, with no `digest`
    /// column at all. It was a database shaped to the hand-copied predicate,
    /// and the canonical statement could not even be evaluated against it.
    fn real_stores(config: &OperatorConfig) -> (BlobStore, WorkspaceManager, ArtifactStore) {
        (
            BlobStore::open(&config.blob_root, config.blob_quota_bytes).unwrap(),
            WorkspaceManager::open(&config.workspace_root, config.workspace_quota_bytes).unwrap(),
            ArtifactStore::open(crate::artifact::metadata_path(&config.database_path)).unwrap(),
        )
    }

    async fn seed_blob(blobs: &BlobStore, bytes: &'static [u8]) -> eggwork_core::BlobDigest {
        let digest = eggwork_core::BlobDigest::from_bytes(bytes);
        blobs
            .put_stream(
                digest.clone(),
                bytes.len() as u64,
                futures_util::stream::iter([Ok::<_, ()>(bytes::Bytes::from_static(bytes))]),
            )
            .await
            .unwrap();
        digest
    }

    /// The blob states the two candidate predicates can disagree about.
    struct BlobStates {
        /// A live reference row: never a candidate.
        live: eggwork_core::BlobDigest,
        /// Only an expired reference row: a candidate once that row is gone.
        /// Absent when the fixture seeds no expired reference at all.
        expired: Option<eggwork_core::BlobDigest>,
        /// No reference rows at all: always a candidate.
        unreferenced: Vec<eggwork_core::BlobDigest>,
        /// A live reference row whose counter says zero.
        ///
        /// Two of them, so that the two candidate predicates disagree on the
        /// *count* and not only on the membership: a counter-based predicate
        /// would report one blob too many, which a count comparison can see.
        counter_low: Vec<eggwork_core::BlobDigest>,
        /// No reference rows whose counter says one.
        counter_high: eggwork_core::BlobDigest,
    }

    /// Seed the blob states and drift the trigger-maintained counter against
    /// the reference rows of two of them.
    ///
    /// `blobs.reference_count` is a denormalization written only by the
    /// `blob_reference_insert` / `blob_reference_delete` triggers. Drifting it
    /// here does not fabricate a reference row: it reproduces the condition
    /// under which a counter-based predicate and a reference-row predicate
    /// answer different questions, which is the whole point of the
    /// divergence this milestone removes.
    async fn seed_blob_states(
        config: &OperatorConfig,
        blobs: &BlobStore,
        with_expired_reference: bool,
    ) -> BlobStates {
        let states = BlobStates {
            live: seed_blob(blobs, b"live").await,
            expired: if with_expired_reference {
                Some(seed_blob(blobs, b"expired").await)
            } else {
                None
            },
            unreferenced: vec![
                seed_blob(blobs, b"unreferenced-a").await,
                seed_blob(blobs, b"unreferenced-b").await,
                seed_blob(blobs, b"unreferenced-c").await,
            ],
            counter_low: vec![
                seed_blob(blobs, b"counter-low-a").await,
                seed_blob(blobs, b"counter-low-b").await,
            ],
            counter_high: seed_blob(blobs, b"counter-high").await,
        };
        blobs
            .retain(
                "workspace",
                "ws-live",
                std::slice::from_ref(&states.live),
                None,
            )
            .unwrap();
        blobs
            .retain("workspace", "ws-counter", &states.counter_low, None)
            .unwrap();
        if let Some(expired) = states.expired.clone() {
            blobs
                .retain("workspace", "ws-expired", &[expired], Some(1))
                .unwrap();
        }
        let connection =
            rusqlite::Connection::open(crate::blob::metadata_path(&config.blob_root)).unwrap();
        for digest in &states.counter_low {
            connection
                .execute(
                    "UPDATE blobs SET reference_count = 0 WHERE digest = ?1",
                    [digest.as_str()],
                )
                .unwrap();
        }
        connection
            .execute(
                "UPDATE blobs SET reference_count = 1 WHERE digest = ?1",
                [states.counter_high.as_str()],
            )
            .unwrap();
        drop(connection);
        states
    }

    /// Seed expired workspaces and artifacts so the preview's workspace and
    /// artifact figures are exercised against real tables.
    ///
    /// The workspace rows need a storage directory that exists, because the
    /// manager's reconciliation drops rows whose tree is gone.
    fn seed_expired_workspaces(config: &OperatorConfig, count: usize) {
        let connection =
            rusqlite::Connection::open(crate::workspace::metadata_path(&config.workspace_root))
                .unwrap();
        for index in 0..count {
            let storage_key = format!("workspace-{}", uuid::Uuid::new_v4());
            fs::create_dir_all(config.workspace_root.join(&storage_key)).unwrap();
            connection
                .execute(
                    "INSERT INTO workspaces(workspace_id, execution_id, generation, principal_id,
                       manifest_digest, storage_key, logical_bytes, state, created_unix_ms,
                       expires_unix_ms)
                     VALUES (?1, 'exec-expired', 1, 'principal-a', 'digest', ?2, 99, 'Ready', 0, 1)",
                    rusqlite::params![
                        format!("workspace-expired-{index}"),
                        storage_key
                    ],
                )
                .unwrap();
        }
        drop(connection);
    }

    fn seed_expired_artifacts(config: &OperatorConfig, count: usize) {
        let connection =
            rusqlite::Connection::open(crate::artifact::metadata_path(&config.database_path))
                .unwrap();
        for index in 0..count {
            connection
                .execute(
                    "INSERT INTO artifacts(artifact_id, execution_id, generation, principal_id,
                       record_json, created_unix_ms, expires_unix_ms)
                     VALUES (?1, 'exec-expired', 1, 'principal-a', ?2, 0, 1)",
                    rusqlite::params![
                        format!("artifact-expired-{index}"),
                        format!("{{\"n\":{index}}}").into_bytes()
                    ],
                )
                .unwrap();
        }
        drop(connection);
    }

    /// The candidates the store's own canonical statement selects.
    fn owner_preview(config: &OperatorConfig, now: u64, limit: usize) -> crate::blob::GcPreview {
        crate::blob::preview_gc(&crate::blob::metadata_path(&config.blob_root), now, limit).unwrap()
    }

    /// The digests the store's own candidate statement selects.
    fn owner_candidates(config: &OperatorConfig, now: u64, limit: usize) -> Vec<String> {
        owner_preview(config, now, limit)
            .candidates
            .into_iter()
            .map(|(digest, _)| digest)
            .collect()
    }

    /// Every seeded digest, in a stable order.
    fn all_seeded(states: &BlobStates) -> Vec<eggwork_core::BlobDigest> {
        let mut all = vec![states.live.clone(), states.counter_high.clone()];
        all.extend(states.counter_low.iter().cloned());
        all.extend(states.expired.clone());
        all.extend(states.unreferenced.iter().cloned());
        all
    }

    /// The digests whose object file the collector removed.
    fn removed_objects(config: &OperatorConfig, states: &BlobStates) -> Vec<String> {
        all_seeded(states)
            .into_iter()
            .filter(|digest| !crate::blob::object_path(&config.blob_root, digest).exists())
            .map(|digest| digest.as_str().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn garbage_collection_preview_reads_only_and_reports_bounds() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
        let (blobs, workspaces, artifacts) = real_stores(&config);
        seed_blob_states(&config, &blobs, true).await;
        seed_expired_workspaces(&config, 2);
        seed_expired_artifacts(&config, 2);
        // Close the stores so the preview reads a cleanly checkpointed
        // database rather than a live write-ahead log.
        drop(artifacts);
        drop(workspaces);
        drop(blobs);
        let in_flight_upload = config.blob_root.join(".part-live-upload");
        fs::write(&in_flight_upload, b"partial upload").unwrap();

        // The bound is honoured: the preview reports a bounded sample of the
        // eligible entries, not a census.
        let bounded = collect_garbage(&config, true, 1).await.unwrap();
        assert_eq!(bounded.workspace_candidates, 1);
        assert_eq!(bounded.artifact_candidates, 1);
        assert_eq!(bounded.blob_candidates, 1);
        assert!(bounded.dry_run);
        assert_eq!(bounded.workspaces_deleted, 0);
        assert_eq!(bounded.artifacts_deleted, 0);
        assert_eq!(bounded.blobs_deleted, 0);
        // A preview reads; it does not sweep. The in-flight upload survives.
        assert!(in_flight_upload.exists());
        assert_eq!(fs::read(&in_flight_upload).unwrap(), b"partial upload");

        // The unbounded figures of the same store.
        let full = collect_garbage(&config, true, 8).await.unwrap();
        assert_eq!(full.workspace_candidates, 2);
        assert_eq!(full.workspace_logical_bytes, 198);
        assert_eq!(full.artifact_candidates, 2);
        assert_eq!(full.expired_blob_references, 1);
        assert_eq!(full.blob_candidates, 4);
        assert!(full.dry_run);
        assert!(in_flight_upload.exists());
        assert_eq!(fs::read(&in_flight_upload).unwrap(), b"partial upload");
    }

    #[tokio::test]
    async fn gc_preview_uses_the_stores_own_blob_predicate_not_the_reference_count_counter() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
        let (blobs, workspaces, artifacts) = real_stores(&config);
        let states = seed_blob_states(&config, &blobs, true).await;
        drop(artifacts);
        drop(workspaces);
        drop(blobs);

        let report = collect_garbage(&config, true, 8).await.unwrap();
        let owner = owner_preview(&config, crate::artifact::now_unix_ms(), 8);
        let candidates: Vec<String> = owner
            .candidates
            .iter()
            .map(|(digest, _)| digest.clone())
            .collect();

        // The operator's preview reports exactly the owner's candidate set -
        // the same count and the same bytes. A counter-based predicate would
        // report two blobs too many here, because it counts the drifted
        // blobs whose counter says zero and misses the one whose counter
        // says one.
        assert_eq!(report.blob_candidates as usize, owner.candidates.len());
        assert_eq!(report.blob_candidates, 4);
        assert_eq!(
            report.blob_candidate_bytes,
            owner
                .candidates
                .iter()
                .fold(0u64, |sum, (_, size)| sum.saturating_add(*size))
        );

        // The reference-row predicate, not the counter: a blob whose counter
        // says zero but which still has a live reference row is not a
        // candidate, and a blob whose counter says one but which has no
        // reference rows is. A counter-based predicate answers exactly the
        // opposite for both, which is the divergence this test rules out.
        for digest in &states.counter_low {
            assert!(
                !candidates.contains(&digest.as_str().to_owned()),
                "a blob with a live reference row is never a candidate"
            );
        }
        assert!(
            candidates.contains(&states.counter_high.as_str().to_owned()),
            "a blob with no reference rows is a candidate whatever the counter says"
        );
        assert!(!candidates.contains(&states.live.as_str().to_owned()));
        assert!(!candidates.contains(&states.expired.as_ref().unwrap().as_str().to_owned()));
        for digest in &states.unreferenced {
            assert!(candidates.contains(&digest.as_str().to_owned()));
        }
    }

    #[tokio::test]
    async fn gc_preview_candidates_are_a_subset_of_what_the_apply_pass_removes() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
        let (blobs, workspaces, artifacts) = real_stores(&config);
        let states = seed_blob_states(&config, &blobs, true).await;
        drop(artifacts);
        drop(workspaces);
        drop(blobs);

        let preview = owner_candidates(&config, crate::artifact::now_unix_ms(), 8);
        assert_eq!(preview.len(), 4);

        let applied = collect_garbage(&config, false, 8).await.unwrap();
        assert!(!applied.dry_run);
        let removed = removed_objects(&config, &states);

        // The preview's set is a subset of what the apply pass removes.
        for digest in &preview {
            assert!(
                removed.contains(digest),
                "the preview reported {digest}, which the apply pass did not remove"
            );
        }
        // The apply pass removes strictly more here, and only for the
        // documented reason: it selects after the expired reference rows are
        // deleted, so a blob that has just become unreferenced is collectable
        // in the same pass.
        assert_eq!(removed.len(), 5);
        assert!(removed.contains(&states.expired.as_ref().unwrap().as_str().to_owned()));
        assert!(!removed.contains(&states.live.as_str().to_owned()));
        for digest in &states.counter_low {
            assert!(!removed.contains(&digest.as_str().to_owned()));
        }
        assert!(!removed.contains(&states.live.as_str().to_owned()));
    }

    #[tokio::test]
    async fn gc_preview_and_apply_agree_when_the_store_has_no_expired_references() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
        let (blobs, workspaces, artifacts) = real_stores(&config);
        let states = seed_blob_states(&config, &blobs, false).await;
        drop(artifacts);
        drop(workspaces);
        drop(blobs);

        let preview = owner_candidates(&config, crate::artifact::now_unix_ms(), 8);
        assert_eq!(preview.len(), 4);

        let applied = collect_garbage(&config, false, 8).await.unwrap();
        assert_eq!(applied.blob_candidates, 4);

        let removed = removed_objects(&config, &states);

        // With no expired references the two passes select at the same point
        // in the transaction, so the sets are equal.
        let mut preview_sorted = preview.clone();
        preview_sorted.sort();
        let mut removed_sorted = removed.clone();
        removed_sorted.sort();
        assert_eq!(preview_sorted, removed_sorted);
    }

    #[tokio::test]
    async fn gc_preview_does_not_create_a_store_that_does_not_exist() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        assert!(!config.blob_root.exists());
        assert!(!config.workspace_root.exists());
        assert!(!config.database_path.exists());
        assert!(!config.database_path.parent().unwrap().exists());

        // The preview cannot evaluate a store that is not there, so it fails
        // with a typed error rather than reporting zero candidates - and it
        // fails without bringing the store into being.
        assert!(collect_garbage(&config, true, 1).await.is_err());
        assert!(!config.blob_root.exists());
        assert!(!config.workspace_root.exists());
        assert!(!config.database_path.exists());
        assert!(!config.database_path.parent().unwrap().exists());

        // The assertion has teeth: the creating, reconciling open the
        // preview must not use does bring the store into being.
        BlobStore::open(&config.blob_root, config.blob_quota_bytes).unwrap();
        assert!(config.blob_root.exists());
        assert!(crate::blob::metadata_path(&config.blob_root).exists());
    }

    #[tokio::test]
    async fn inspect_blob_resolves_the_object_through_the_store_path_ownership() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        let blobs = BlobStore::open(&config.blob_root, config.blob_quota_bytes).unwrap();
        let digest = seed_blob(&blobs, b"inspectable").await;

        let inspection = inspect_blob(&config, digest.as_str()).unwrap();
        assert_eq!(inspection.kind, "blob");
        assert_eq!(inspection.id, digest.as_str());
        assert_eq!(inspection.metadata["size_bytes"], 11u64);

        // A recorded size that disagrees with the file's actual length is a
        // data error, not a successful inspection.
        let connection =
            rusqlite::Connection::open(crate::blob::metadata_path(&config.blob_root)).unwrap();
        connection
            .execute(
                "UPDATE blobs SET size_bytes = 3 WHERE digest = ?1",
                [digest.as_str()],
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            inspect_blob(&config, digest.as_str()),
            Err(OperationsError::Data)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn inspect_blob_refuses_a_symlinked_shard_or_object() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        let blobs = BlobStore::open(&config.blob_root, config.blob_quota_bytes).unwrap();
        let digest = seed_blob(&blobs, b"inspectable").await;
        let shard = crate::blob::shard_path(&config.blob_root, &digest);
        let object = crate::blob::object_path(&config.blob_root, &digest);

        // A symlinked object file is refused.
        let bytes = fs::read(&object).unwrap();
        let outside = temp.path().join("outside-object");
        fs::write(&outside, &bytes).unwrap();
        fs::remove_file(&object).unwrap();
        symlink(&outside, &object).unwrap();
        assert!(matches!(
            inspect_blob(&config, digest.as_str()),
            Err(OperationsError::NotFound)
        ));

        // A symlinked shard directory is refused.
        fs::remove_file(&object).unwrap();
        fs::write(&object, &bytes).unwrap();
        let outside_shard = temp.path().join("outside-shard");
        fs::create_dir_all(&outside_shard).unwrap();
        fs::write(outside_shard.join(digest.as_str()), &bytes).unwrap();
        fs::remove_dir_all(&shard).unwrap();
        symlink(&outside_shard, &shard).unwrap();
        assert!(matches!(
            inspect_blob(&config, digest.as_str()),
            Err(OperationsError::NotFound)
        ));
    }

    #[tokio::test]
    async fn applying_gc_refuses_to_race_a_running_node() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
        let _node_lock = acquire_maintenance_lock(&config.database_path).unwrap();
        assert!(collect_garbage(&config, false, 1).await.is_err());
    }

    #[test]
    fn metrics_report_uses_a_fixed_bounded_counter_set() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(&config.database_path).unwrap();
        db.execute_batch("CREATE TABLE node_metrics(name TEXT PRIMARY KEY,value INTEGER);")
            .unwrap();
        db.execute(
            "INSERT INTO node_metrics VALUES ('executions_accepted',7)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO node_metrics VALUES ('untrusted_raw_execution_id',99)",
            [],
        )
        .unwrap();
        drop(db);
        let metrics = metrics_snapshot(&config).unwrap();
        assert_eq!(metrics.counters.get("executions_accepted"), Some(&7));
        assert!(!metrics.counters.contains_key("untrusted_raw_execution_id"));
        assert_eq!(metrics.counters.len(), METRIC_NAMES.len());
    }

    #[tokio::test]
    async fn invalid_configuration_is_rejected_without_exposing_secret_material() {
        let temp = TempDir::new().unwrap();
        let mut config = config(temp.path());
        config.clients[0].operations = vec!["allow-everything".into()];
        assert!(config.validate().is_err());
        let report = doctor(&config).await;
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!encoded.contains("super-secret-key.pem"));
        assert!(!encoded.contains(&"ab".repeat(32)));
        assert!(!report.ready);
    }

    #[cfg(unix)]
    #[test]
    fn state_directories_must_be_exactly_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let directory = temp.path().join("state");
        fs::create_dir(&directory).unwrap();
        let metadata = |mode: u32| {
            fs::set_permissions(&directory, fs::Permissions::from_mode(mode)).unwrap();
            fs::symlink_metadata(&directory).unwrap()
        };
        assert!(trusted_directory(&metadata(0o700)));
        // The published rule is owner-only `0700`. `0750`/`0755` leave the whole
        // execution output, workspace and blob surface readable by other local
        // users while the configuration still validates.
        assert!(!trusted_directory(&metadata(0o750)));
        assert!(!trusted_directory(&metadata(0o755)));
        assert!(!trusted_directory(&metadata(0o500)));
        assert!(!trusted_directory(&metadata(0o707)));
    }

    #[test]
    fn valid_operator_config_builds_required_mtls_server_identity() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        for directory in ["execution", "state", "blobs", "workspaces"] {
            let path = root.join(directory);
            fs::create_dir(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let identity = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let certificate_path = root.join("server.pem");
        let ca_path = root.join("client-ca.pem");
        let key_path = root.join("private-key.pem");
        fs::write(&certificate_path, identity.cert.pem()).unwrap();
        fs::write(&ca_path, identity.cert.pem()).unwrap();
        fs::write(&key_path, identity.key_pair.serialize_pem()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::set_permissions(&certificate_path, fs::Permissions::from_mode(0o644)).unwrap();
            fs::set_permissions(&ca_path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let mut config = config(root);
        config.execution_root = root.join("execution");
        config.database_path = root.join("state/executions.sqlite");
        config.blob_root = root.join("blobs");
        config.workspace_root = root.join("workspaces");
        config.tls.certificate_chain = certificate_path;
        config.tls.private_key = key_path;
        config.tls.client_ca = ca_path;
        let config_path = root.join("node.json");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config_path, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let loaded = OperatorConfig::load(&config_path).unwrap();
        loaded.validate().unwrap();
        assert_eq!(
            loaded.node_config().unwrap().tls.client_auth(),
            eggserve_core::tls::ClientAuthMode::Required
        );
    }
}
