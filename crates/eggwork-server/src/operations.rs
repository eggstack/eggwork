//! Local operator configuration and bounded inspection/maintenance helpers.

use crate::{
    Authorizer, FingerprintPrincipalResolver, NodeConfig, NodeGcReport, NodePrincipal, NodeServer,
    Operation, artifact::ArtifactStore, blob::BlobStore, workspace::WorkspaceManager,
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
        for path in [&self.execution_root, &self.blob_root, &self.workspace_root] {
            let meta = fs::symlink_metadata(path).map_err(|_| OperationsError::Config)?;
            if !meta.is_dir() || meta.file_type().is_symlink() || !trusted_directory(&meta) {
                return Err(OperationsError::Config);
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
    metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.permissions().mode() & 0o700 == 0o700
        && metadata.permissions().mode() & 0o022 == 0
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

#[cfg(unix)]
fn trusted_helper(path: &Path) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o022 == 0
            && metadata.permissions().mode() & 0o111 != 0
    })
}

#[cfg(not(unix))]
fn trusted_helper(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
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
    let artifact_db = config.database_path.with_extension("artifacts.sqlite");
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
    let artifacts = ArtifactStore::open(config.database_path.with_extension("artifacts.sqlite"))
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
    let now = crate::artifact::now_unix_ms() as i64;
    let workspace_db = read_only_database(&config.workspace_root.join("metadata.sqlite"))?;
    let (workspace_candidates, workspace_logical_bytes): (i64, i64) = workspace_db
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(logical_bytes),0) FROM
             (SELECT logical_bytes FROM workspaces WHERE expires_unix_ms IS NOT NULL
              AND expires_unix_ms <= ?1 ORDER BY expires_unix_ms LIMIT ?2)",
            rusqlite::params![now, limit as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| OperationsError::Data)?;
    // Older stores predate the retained-manifest cache; report zero instead
    // of failing the whole preview.
    let manifest_candidates: i64 = workspace_db
        .query_row(
            "SELECT COUNT(*) FROM (SELECT manifest_digest FROM retained_manifests
             WHERE expires_unix_ms IS NOT NULL AND expires_unix_ms <= ?1
             ORDER BY expires_unix_ms LIMIT ?2)",
            rusqlite::params![now, limit as i64],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let artifact_db = read_only_database(&config.database_path.with_extension("artifacts.sqlite"))?;
    let artifact_candidates: i64 = artifact_db
        .query_row(
            "SELECT COUNT(*) FROM (SELECT artifact_id FROM artifacts
             WHERE expires_unix_ms <= ?1 ORDER BY expires_unix_ms LIMIT ?2)",
            rusqlite::params![now, limit as i64],
            |row| row.get(0),
        )
        .map_err(|_| OperationsError::Data)?;
    let blob_db = read_only_database(&config.blob_root.join("metadata.sqlite"))?;
    let expired_blob_references: i64 = blob_db
        .query_row(
            "SELECT COUNT(*) FROM blob_references WHERE expires_unix_ms IS NOT NULL AND expires_unix_ms <= ?1",
            [now],
            |row| row.get(0),
        )
        .map_err(|_| OperationsError::Data)?;
    let (blob_candidates, blob_candidate_bytes): (i64, i64) = blob_db
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes),0) FROM
             (SELECT size_bytes FROM blobs WHERE reference_count = 0 ORDER BY last_used_unix_ms LIMIT ?1)",
            [limit as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| OperationsError::Data)?;
    Ok(NodeGcReport {
        workspace_candidates: workspace_candidates.max(0) as u64,
        workspace_logical_bytes: workspace_logical_bytes.max(0) as u64,
        workspaces_deleted: 0,
        manifest_candidates: manifest_candidates.max(0) as u64,
        manifests_deleted: 0,
        artifact_candidates: artifact_candidates.max(0) as u64,
        artifacts_deleted: 0,
        expired_blob_references: expired_blob_references.max(0) as u64,
        blob_candidates: blob_candidates.max(0) as u64,
        blob_candidate_bytes: blob_candidate_bytes.max(0) as u64,
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
    let shard = config.blob_root.join(&digest.as_str()[..2]);
    let shard_meta = fs::symlink_metadata(&shard).map_err(|_| OperationsError::NotFound)?;
    if !shard_meta.is_dir() || shard_meta.file_type().is_symlink() {
        return Err(OperationsError::NotFound);
    }
    let path = shard.join(digest.as_str());
    let metadata = fs::symlink_metadata(&path).map_err(|_| OperationsError::NotFound)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OperationsError::NotFound);
    }
    let db = read_only_database(&config.blob_root.join("metadata.sqlite"))?;
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
    let db = read_only_database(&config.workspace_root.join("metadata.sqlite"))?;
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
    let db = read_only_database(&config.database_path.with_extension("artifacts.sqlite"))?;
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

    #[tokio::test]
    async fn garbage_collection_preview_reads_only_and_reports_bounds() {
        let temp = TempDir::new().unwrap();
        let config = config(temp.path());
        fs::create_dir_all(&config.workspace_root).unwrap();
        fs::create_dir_all(&config.blob_root).unwrap();
        fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
        let workspace_db =
            rusqlite::Connection::open(config.workspace_root.join("metadata.sqlite")).unwrap();
        workspace_db.execute_batch("CREATE TABLE workspaces(workspace_id TEXT,logical_bytes INTEGER,expires_unix_ms INTEGER);").unwrap();
        workspace_db
            .execute("INSERT INTO workspaces VALUES ('workspace-1',99,0)", [])
            .unwrap();
        drop(workspace_db);
        let artifact_db =
            rusqlite::Connection::open(config.database_path.with_extension("artifacts.sqlite"))
                .unwrap();
        artifact_db
            .execute_batch("CREATE TABLE artifacts(artifact_id TEXT,expires_unix_ms INTEGER);")
            .unwrap();
        artifact_db
            .execute("INSERT INTO artifacts VALUES ('artifact-1',0)", [])
            .unwrap();
        drop(artifact_db);
        let blob_db = rusqlite::Connection::open(config.blob_root.join("metadata.sqlite")).unwrap();
        blob_db.execute_batch("CREATE TABLE blob_references(expires_unix_ms INTEGER); CREATE TABLE blobs(reference_count INTEGER,size_bytes INTEGER,last_used_unix_ms INTEGER);").unwrap();
        blob_db
            .execute("INSERT INTO blob_references VALUES (0)", [])
            .unwrap();
        blob_db
            .execute("INSERT INTO blobs VALUES (0,123,0)", [])
            .unwrap();
        drop(blob_db);
        let in_flight_upload = config.blob_root.join(".part-live-upload");
        fs::write(&in_flight_upload, b"partial upload").unwrap();

        let report = collect_garbage(&config, true, 1).await.unwrap();
        assert_eq!(report.workspace_candidates, 1);
        assert_eq!(report.workspace_logical_bytes, 99);
        assert_eq!(report.artifact_candidates, 1);
        assert_eq!(report.expired_blob_references, 1);
        assert_eq!(report.blob_candidates, 1);
        assert!(report.dry_run);
        assert!(in_flight_upload.exists());
        assert_eq!(fs::read(&in_flight_upload).unwrap(), b"partial upload");
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
