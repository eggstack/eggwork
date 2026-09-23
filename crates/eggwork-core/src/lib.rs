#![forbid(unsafe_code)]

//! Protocol-neutral bounded domain types for Eggwork.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, time::Duration};
use thiserror::Error;

pub const MAX_ID_BYTES: usize = 128;
pub const MAX_ARG_COUNT: usize = 256;
pub const MAX_ARG_BYTES: usize = 32 * 1024;
pub const MAX_ENV_COUNT: usize = 256;
pub const MAX_ENV_NAME_BYTES: usize = 256;
pub const MAX_ENV_VALUE_BYTES: usize = 16 * 1024;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_METADATA_COUNT: usize = 64;
pub const MAX_METADATA_KEY_BYTES: usize = 128;
pub const MAX_METADATA_VALUE_BYTES: usize = 2048;
pub const MAX_OUTPUTS: usize = 128;
pub const MAX_STDIN_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EVENT_CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TIMEOUT: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const MAX_CAPABILITIES: usize = 256;
pub const MAX_WORKSPACE_ENTRIES: usize = 4096;
pub const MAX_WORKSPACE_PATH_BYTES: usize = 1024 * 1024;
pub const MAX_WORKSPACE_DEPTH: usize = 128;
pub const MAX_WORKSPACE_LOGICAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("{field} must not be empty")]
    Empty { field: &'static str },
    #[error("{field} exceeds its {max}-byte limit")]
    TooLong { field: &'static str, max: usize },
    #[error("{field} has invalid syntax")]
    InvalidSyntax { field: &'static str },
    #[error("{field} exceeds its {max}-item limit")]
    TooMany { field: &'static str, max: usize },
    #[error("{field} contains a forbidden value")]
    Forbidden { field: &'static str },
    #[error("{field} must be relative and confined")]
    InvalidPath { field: &'static str },
    #[error("{field} is outside its supported range")]
    OutOfRange { field: &'static str },
}

macro_rules! id_type {
    ($name:ident, $label:literal) => {
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
                let value = value.into();
                if value.is_empty() {
                    return Err(ValidationError::Empty { field: $label });
                }
                if value.len() > MAX_ID_BYTES {
                    return Err(ValidationError::TooLong {
                        field: $label,
                        max: MAX_ID_BYTES,
                    });
                }
                if !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
                {
                    return Err(ValidationError::InvalidSyntax { field: $label });
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = ValidationError;
            fn try_from(v: String) -> Result<Self, Self::Error> {
                Self::new(v)
            }
        }
        impl From<$name> for String {
            fn from(v: $name) -> Self {
                v.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.0).finish()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(NodeId, "node id");
id_type!(PrincipalId, "principal id");
id_type!(ExecutionId, "execution id");
id_type!(LeaseId, "lease id");
id_type!(WorkspaceId, "workspace id");
id_type!(ArtifactId, "artifact id");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionGeneration(u64);
impl ExecutionGeneration {
    pub fn new(value: u64) -> Result<Self, ValidationError> {
        if value == 0 {
            Err(ValidationError::OutOfRange {
                field: "execution generation",
            })
        } else {
            Ok(Self(value))
        }
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSequence(u64);
impl EventSequence {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BlobDigest(String);
impl BlobDigest {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(ValidationError::InvalidSyntax {
                field: "sha256 blob digest",
            });
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for BlobDigest {
    type Error = ValidationError;
    fn try_from(v: String) -> Result<Self, Self::Error> {
        Self::parse(v)
    }
}
impl From<BlobDigest> for String {
    fn from(v: BlobDigest) -> Self {
        v.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersionRange {
    pub min: ProtocolVersion,
    pub max: ProtocolVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelativePath(String);
impl RelativePath {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PATH_BYTES
            || value.starts_with('/')
            || value.contains('\\')
            || value.as_bytes().contains(&0)
        {
            return Err(ValidationError::InvalidPath {
                field: "relative path",
            });
        }
        if value
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
        {
            return Err(ValidationError::InvalidPath {
                field: "relative path",
            });
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for RelativePath {
    type Error = ValidationError;
    fn try_from(v: String) -> Result<Self, Self::Error> {
        Self::new(v)
    }
}
impl From<RelativePath> for String {
    fn from(v: RelativePath) -> Self {
        v.0
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentEntry {
    pub name: String,
    pub value: String,
}
impl fmt::Debug for EnvironmentEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvironmentEntry")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}
impl EnvironmentEntry {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Result<Self, ValidationError> {
        let (name, value) = (name.into(), value.into());
        if name.is_empty()
            || name.len() > MAX_ENV_NAME_BYTES
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || name.as_bytes().first().is_some_and(u8::is_ascii_digit)
            || name.as_bytes().contains(&0)
        {
            return Err(ValidationError::InvalidSyntax {
                field: "environment name",
            });
        }
        if value.len() > MAX_ENV_VALUE_BYTES {
            return Err(ValidationError::TooLong {
                field: "environment value",
                max: MAX_ENV_VALUE_BYTES,
            });
        }
        if value.as_bytes().contains(&0) {
            return Err(ValidationError::Forbidden {
                field: "environment value NUL",
            });
        }
        Ok(Self { name, value })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StdinPolicy {
    Null,
    Bytes(Vec<u8>),
}
impl fmt::Debug for StdinPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Self::Bytes(v) = self
            && v.len() > MAX_STDIN_BYTES
        {
            return Err(ValidationError::TooLong {
                field: "stdin",
                max: MAX_STDIN_BYTES,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverflowPolicy {
    Truncate,
    Terminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputPolicy {
    pub capture_limit_bytes: usize,
    pub event_chunk_bytes: usize,
    pub overflow: OverflowPolicy,
}
impl Default for OutputPolicy {
    fn default() -> Self {
        Self {
            capture_limit_bytes: 1024 * 1024,
            event_chunk_bytes: 16 * 1024,
            overflow: OverflowPolicy::Truncate,
        }
    }
}
impl OutputPolicy {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.capture_limit_bytes > MAX_CAPTURE_BYTES
            || self.event_chunk_bytes == 0
            || self.event_chunk_bytes > MAX_EVENT_CHUNK_BYTES
        {
            return Err(ValidationError::OutOfRange {
                field: "output policy",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredOutput {
    pub path: RelativePath,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRequirements {
    #[serde(default)]
    pub memory_bytes: Requirement<u64>,
    #[serde(default)]
    pub cpu_millis: Requirement<u64>,
    #[serde(default)]
    pub pids: Requirement<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement<T> {
    NotRequested,
    BestEffort(T),
    Required(T),
}

impl<T> Default for Requirement<T> {
    fn default() -> Self {
        Self::NotRequested
    }
}

impl<T: Serialize> Serialize for Requirement<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::NotRequested => serializer.serialize_none(),
            Self::BestEffort(value) => value.serialize(serializer),
            Self::Required(value) => {
                #[derive(Serialize)]
                struct Required<'a, T> {
                    required: &'a T,
                }
                Required { required: value }.serialize(serializer)
            }
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Requirement<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_null() {
            return Ok(Self::NotRequested);
        }
        if let Some(object) = value.as_object()
            && object.len() == 1
            && let Some(required) = object.get("required")
        {
            return T::deserialize(required.clone())
                .map(Self::Required)
                .map_err(serde::de::Error::custom);
        }
        T::deserialize(value)
            .map(Self::BestEffort)
            .map_err(serde::de::Error::custom)
    }
}

impl ResourceRequirements {
    pub fn validate(&self) -> Result<(), ValidationError> {
        let memory_valid = matches!(
            self.memory_bytes,
            Requirement::NotRequested
                | Requirement::BestEffort(1..=1_125_899_906_842_624)
                | Requirement::Required(1..=1_125_899_906_842_624)
        );
        let cpu_valid = matches!(
            self.cpu_millis,
            Requirement::NotRequested
                | Requirement::BestEffort(1..=1_000_000)
                | Requirement::Required(1..=1_000_000)
        );
        let pids_valid = matches!(
            self.pids,
            Requirement::NotRequested
                | Requirement::BestEffort(1..=1_000_000)
                | Requirement::Required(1..=1_000_000)
        );
        if memory_valid && cpu_valid && pids_valid {
            Ok(())
        } else {
            Err(ValidationError::OutOfRange {
                field: "resource requirements",
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceDimension {
    Memory,
    Cpu,
    Pids,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsolationRequirement {
    None,
    BestEffort,
    Required,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkRequirement {
    Unrestricted,
    Disabled,
    AllowListed(Vec<String>),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub cwd: Option<RelativePath>,
    pub environment: Vec<EnvironmentEntry>,
    pub stdin: StdinPolicy,
    pub timeout_millis: u64,
    pub output: OutputPolicy,
    pub declared_outputs: Vec<DeclaredOutput>,
    pub resources: ResourceRequirements,
    pub isolation: IsolationRequirement,
    pub network: NetworkRequirement,
}
impl fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandSpec")
            .field("argv", &"[REDACTED]")
            .field("cwd", &self.cwd)
            .field("environment", &self.environment)
            .field("stdin", &self.stdin)
            .field("timeout_millis", &self.timeout_millis)
            .field("output", &self.output)
            .field("declared_outputs", &self.declared_outputs)
            .field("resources", &self.resources)
            .field("isolation", &self.isolation)
            .field("network", &self.network)
            .finish()
    }
}
impl CommandSpec {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.argv.is_empty() {
            return Err(ValidationError::Empty { field: "argv" });
        }
        if self.argv.len() > MAX_ARG_COUNT {
            return Err(ValidationError::TooMany {
                field: "argv",
                max: MAX_ARG_COUNT,
            });
        }
        if self
            .argv
            .iter()
            .any(|a| a.len() > MAX_ARG_BYTES || a.as_bytes().contains(&0))
        {
            return Err(ValidationError::OutOfRange {
                field: "argv entry",
            });
        }
        if self.environment.len() > MAX_ENV_COUNT {
            return Err(ValidationError::TooMany {
                field: "environment",
                max: MAX_ENV_COUNT,
            });
        }
        for e in &self.environment {
            EnvironmentEntry::new(&e.name, &e.value)?;
        }
        let mut environment_names =
            std::collections::HashSet::with_capacity(self.environment.len());
        if self
            .environment
            .iter()
            .any(|entry| !environment_names.insert(entry.name.as_str()))
        {
            return Err(ValidationError::InvalidSyntax {
                field: "duplicate environment name",
            });
        }
        if self.declared_outputs.len() > MAX_OUTPUTS {
            return Err(ValidationError::TooMany {
                field: "declared outputs",
                max: MAX_OUTPUTS,
            });
        }
        if self.timeout_millis == 0 || self.timeout_millis > MAX_TIMEOUT.as_millis() as u64 {
            return Err(ValidationError::OutOfRange { field: "timeout" });
        }
        self.stdin.validate()?;
        self.output.validate()?;
        self.resources.validate()?;
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSpec {
    pub schema_version: u16,
    pub command: CommandSpec,
    pub metadata: Vec<(String, String)>,
}
impl fmt::Debug for ExecutionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionSpec")
            .field("schema_version", &self.schema_version)
            .field("command", &self.command)
            .field("metadata_count", &self.metadata.len())
            .finish()
    }
}
impl ExecutionSpec {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.command.validate()?;
        if self.schema_version == 0 {
            return Err(ValidationError::OutOfRange {
                field: "schema version",
            });
        }
        if self.metadata.len() > MAX_METADATA_COUNT {
            return Err(ValidationError::TooMany {
                field: "metadata",
                max: MAX_METADATA_COUNT,
            });
        }
        if self.metadata.iter().any(|(k, v)| {
            k.is_empty()
                || k.len() > MAX_METADATA_KEY_BYTES
                || v.len() > MAX_METADATA_VALUE_BYTES
                || k.as_bytes().contains(&0)
                || v.as_bytes().contains(&0)
        }) {
            return Err(ValidationError::OutOfRange {
                field: "metadata entry",
            });
        }
        Ok(())
    }
}

/// Portable file-tree description. Version 1 uses conservative ASCII paths
/// and rejects symlinks so materialization stays identical across hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub schema_version: u16,
    pub entries: Vec<WorkspaceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEntry {
    Directory {
        path: RelativePath,
    },
    File {
        path: RelativePath,
        digest: BlobDigest,
        size_bytes: u64,
        executable: bool,
    },
    Symlink {
        path: RelativePath,
        target: String,
    },
}

impl WorkspaceEntry {
    pub fn path(&self) -> &RelativePath {
        match self {
            Self::Directory { path } | Self::File { path, .. } | Self::Symlink { path, .. } => path,
        }
    }
}

impl WorkspaceManifest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != 1 {
            return Err(ValidationError::OutOfRange {
                field: "workspace schema version",
            });
        }
        if self.entries.len() > MAX_WORKSPACE_ENTRIES {
            return Err(ValidationError::TooMany {
                field: "workspace entries",
                max: MAX_WORKSPACE_ENTRIES,
            });
        }
        let mut paths = std::collections::BTreeMap::<&str, bool>::new();
        let mut folded = std::collections::HashSet::new();
        let mut total_path_bytes = 0usize;
        let mut total_file_bytes = 0u64;
        for entry in &self.entries {
            let raw_path = entry.path().as_str();
            validate_portable_workspace_path(raw_path)?;
            total_path_bytes = total_path_bytes.saturating_add(raw_path.len());
            if total_path_bytes > MAX_WORKSPACE_PATH_BYTES {
                return Err(ValidationError::OutOfRange {
                    field: "workspace path bytes",
                });
            }
            if !folded.insert(raw_path.to_ascii_lowercase()) {
                return Err(ValidationError::InvalidSyntax {
                    field: "workspace case collision",
                });
            }
            let is_directory = matches!(entry, WorkspaceEntry::Directory { .. });
            if paths.insert(raw_path, is_directory).is_some() {
                return Err(ValidationError::InvalidSyntax {
                    field: "duplicate workspace path",
                });
            }
            match entry {
                WorkspaceEntry::File { size_bytes, .. } => {
                    total_file_bytes = total_file_bytes.saturating_add(*size_bytes);
                    if total_file_bytes > MAX_WORKSPACE_LOGICAL_BYTES {
                        return Err(ValidationError::OutOfRange {
                            field: "workspace logical bytes",
                        });
                    }
                }
                WorkspaceEntry::Symlink { .. } => {
                    return Err(ValidationError::Forbidden {
                        field: "workspace symlink",
                    });
                }
                WorkspaceEntry::Directory { .. } => {}
            }
        }
        for entry in &self.entries {
            let path = entry.path().as_str();
            let mut parent_end = 0;
            while let Some(relative) = path[parent_end..].find('/') {
                parent_end += relative;
                let parent = &path[..parent_end];
                if paths.get(parent) != Some(&true) {
                    return Err(ValidationError::InvalidPath {
                        field: "workspace parent directory",
                    });
                }
                parent_end += 1;
            }
        }
        Ok(())
    }

    pub fn logical_bytes(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| match entry {
                WorkspaceEntry::File { size_bytes, .. } => *size_bytes,
                _ => 0,
            })
            .fold(0u64, u64::saturating_add)
    }

    pub fn digest(&self) -> Result<BlobDigest, serde_json::Error> {
        let mut normalized = self.clone();
        normalized
            .entries
            .sort_by(|a, b| a.path().as_str().cmp(b.path().as_str()));
        let mut bytes = b"eggwork-workspace-manifest\0canonical-json-v1\0".to_vec();
        bytes.extend(serde_json::to_vec(&normalized)?);
        Ok(BlobDigest::from_bytes(&bytes))
    }
}

fn validate_portable_workspace_path(path: &str) -> Result<(), ValidationError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.split('/').count() > MAX_WORKSPACE_DEPTH
        || !path.is_ascii()
        || path.starts_with('/')
        || path.contains('\\')
        || path.as_bytes().contains(&0)
    {
        return Err(ValidationError::InvalidPath {
            field: "portable workspace path",
        });
    }
    for component in path.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with('.')
            || component.ends_with(' ')
            || component.bytes().any(|byte| {
                byte < 0x20 || matches!(byte, b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*')
            })
        {
            return Err(ValidationError::InvalidPath {
                field: "portable workspace path",
            });
        }
        let basename = component
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(
            basename.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
        ) || basename
            .strip_prefix("COM")
            .or_else(|| basename.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
        {
            return Err(ValidationError::InvalidPath {
                field: "reserved workspace path",
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionState {
    Accepted,
    Preparing,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionRejection {
    Busy,
    Draining,
    CapabilityMismatch,
    Unauthorized,
    InvalidRequest,
    Conflict,
    StorageExhausted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionFailure {
    Spawn,
    Internal,
    OutputLimit,
    Sandbox,
    ResourceLimit,
    Interrupted,
    LeaseExpired,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SandboxResult {
    NotRequested,
    NotApplied { reason: String },
    Applied { profile: String },
    Failed { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceDimensionResult {
    NotRequested,
    NotApplied { reason: String },
    Applied { backend: String },
    LimitExceeded { backend: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceResult {
    pub memory_bytes: ResourceDimensionResult,
    pub cpu_millis: ResourceDimensionResult,
    pub pids: ResourceDimensionResult,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionFinalizationFailure {
    ArtifactCapture,
    Retention,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub state: ExecutionState,
    pub exit_code: Option<i32>,
    pub failure: Option<ExecutionFailure>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_omitted: u64,
    pub stderr_omitted: u64,
    pub cleanup_warning: Option<String>,
    #[serde(default)]
    pub finalization_failure: Option<ExecutionFinalizationFailure>,
    #[serde(default)]
    pub artifact_count: u32,
    #[serde(default)]
    pub sandbox: Option<SandboxResult>,
    #[serde(default)]
    pub resources: Option<ResourceResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub artifact_id: ArtifactId,
    pub execution_id: ExecutionId,
    pub generation: ExecutionGeneration,
    pub path: RelativePath,
    pub kind: ArtifactType,
    pub digest: BlobDigest,
    pub size_bytes: u64,
    pub executable: bool,
    pub created_unix_ms: u64,
    pub expires_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    File,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMetadata {
    pub fields: Vec<(String, String)>,
}
impl fmt::Debug for EventMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventMetadata")
            .field("field_count", &self.fields.len())
            .field("values", &"[REDACTED]")
            .finish()
    }
}
impl EventMetadata {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.fields.len() > MAX_METADATA_COUNT {
            return Err(ValidationError::TooMany {
                field: "event metadata",
                max: MAX_METADATA_COUNT,
            });
        }
        if self
            .fields
            .iter()
            .any(|(k, v)| k.len() > MAX_METADATA_KEY_BYTES || v.len() > MAX_METADATA_VALUE_BYTES)
        {
            return Err(ValidationError::OutOfRange {
                field: "event metadata",
            });
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionEventKind {
    State(ExecutionState),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Diagnostic(String),
}
impl fmt::Debug for ExecutionEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(state) => f.debug_tuple("State").field(state).finish(),
            Self::Stdout(bytes) => f
                .debug_struct("Stdout")
                .field("length", &bytes.len())
                .field("bytes", &"[REDACTED]")
                .finish(),
            Self::Stderr(bytes) => f
                .debug_struct("Stderr")
                .field("length", &bytes.len())
                .field("bytes", &"[REDACTED]")
                .finish(),
            Self::Diagnostic(value) => f
                .debug_struct("Diagnostic")
                .field("length", &value.len())
                .field("value", &"[REDACTED]")
                .finish(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEvent {
    pub sequence: EventSequence,
    pub kind: ExecutionEventKind,
    pub metadata: EventMetadata,
}
impl ExecutionEvent {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.metadata.validate()?;
        let n = match &self.kind {
            ExecutionEventKind::Stdout(v) | ExecutionEventKind::Stderr(v) => v.len(),
            ExecutionEventKind::Diagnostic(v) => v.len(),
            _ => 0,
        };
        if n > MAX_EVENT_CHUNK_BYTES {
            return Err(ValidationError::TooLong {
                field: "event payload",
                max: MAX_EVENT_CHUNK_BYTES,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeCapabilities {
    pub protocol: ProtocolVersionRange,
    pub features: Vec<String>,
    pub max_active_executions: u32,
}
impl NodeCapabilities {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.protocol.min > self.protocol.max {
            return Err(ValidationError::OutOfRange {
                field: "protocol version range",
            });
        }
        if self.features.len() > MAX_CAPABILITIES {
            return Err(ValidationError::TooMany {
                field: "capabilities",
                max: MAX_CAPABILITIES,
            });
        }
        if self.features.iter().any(|f| {
            f.is_empty() || f.len() > MAX_METADATA_VALUE_BYTES || f.as_bytes().contains(&0)
        }) {
            return Err(ValidationError::OutOfRange {
                field: "capability",
            });
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node_id: NodeId,
    pub draining: bool,
    pub active_executions: u32,
    pub capabilities: NodeCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Versioned<T> {
    pub schema_version: u16,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteRequest {
    pub schema_version: u16,
    pub spec: ExecutionSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionAccepted {
    pub schema_version: u16,
    pub execution_id: ExecutionId,
    pub state: ExecutionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSnapshot {
    pub schema_version: u16,
    pub execution_id: ExecutionId,
    pub generation: ExecutionGeneration,
    pub state: ExecutionState,
    pub result: Option<ExecutionResult>,
}

/// Fenced handle for control operations on one execution generation.
/// The lease token is bearer authority and is omitted from Debug output.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionHandle {
    pub execution_id: ExecutionId,
    pub generation: ExecutionGeneration,
    pub lease_id: LeaseId,
}
impl fmt::Debug for ExecutionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionHandle")
            .field("execution_id", &self.execution_id)
            .field("generation", &self.generation)
            .field("lease_id", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub schema_version: u16,
    pub code: String,
    pub message: String,
}

/// Version of the execution request canonicalization algorithm.
pub const CANONICAL_REQUEST_VERSION: u16 = 2;
/// Version used when a fixed workspace identity participates in execution identity.
pub const CANONICAL_WORKSPACE_REQUEST_VERSION: u16 = 3;

/// Stable canonical digest input: normalize unordered fields, then hash versioned JSON.
pub fn request_digest(spec: &ExecutionSpec) -> Result<BlobDigest, serde_json::Error> {
    let normalized = normalize_execution_spec(spec);
    let mut bytes = b"eggwork-execution-spec\0canonical-json-v2\0".to_vec();
    bytes.extend(serde_json::to_vec(&normalized)?);
    Ok(BlobDigest::from_bytes(&bytes))
}

pub fn request_digest_with_workspace(
    spec: &ExecutionSpec,
    workspace_id: Option<&WorkspaceId>,
) -> Result<(u16, BlobDigest), serde_json::Error> {
    let Some(workspace_id) = workspace_id else {
        return Ok((CANONICAL_REQUEST_VERSION, request_digest(spec)?));
    };
    #[derive(Serialize)]
    struct CanonicalWorkspaceRequest<'a> {
        spec: &'a ExecutionSpec,
        workspace_id: &'a WorkspaceId,
    }
    let normalized = normalize_execution_spec(spec);
    let mut bytes = b"eggwork-execution-spec\0canonical-json-v3\0".to_vec();
    bytes.extend(serde_json::to_vec(&CanonicalWorkspaceRequest {
        spec: &normalized,
        workspace_id,
    })?);
    Ok((
        CANONICAL_WORKSPACE_REQUEST_VERSION,
        BlobDigest::from_bytes(&bytes),
    ))
}

fn normalize_execution_spec(spec: &ExecutionSpec) -> ExecutionSpec {
    let mut normalized = spec.clone();
    normalized
        .command
        .environment
        .sort_by(|a, b| a.name.cmp(&b.name));
    normalized
        .metadata
        .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    normalized.command.declared_outputs.sort_by(|a, b| {
        a.path
            .as_str()
            .cmp(b.path.as_str())
            .then_with(|| a.required.cmp(&b.required))
    });
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> ExecutionSpec {
        ExecutionSpec {
            schema_version: 1,
            command: CommandSpec {
                argv: vec!["echo".into(), "hi".into()],
                cwd: Some(RelativePath::new("work").unwrap()),
                environment: vec![EnvironmentEntry::new("LANG", "C.UTF-8").unwrap()],
                stdin: StdinPolicy::Null,
                timeout_millis: 1000,
                output: OutputPolicy::default(),
                declared_outputs: vec![],
                resources: ResourceRequirements {
                    memory_bytes: Requirement::NotRequested,
                    cpu_millis: Requirement::NotRequested,
                    pids: Requirement::NotRequested,
                },
                isolation: IsolationRequirement::None,
                network: NetworkRequirement::Unrestricted,
            },
            metadata: vec![],
        }
    }
    #[test]
    fn typed_ids_and_digest_validate() {
        assert!(NodeId::new("node_1").is_ok());
        assert!(NodeId::new("../bad").is_err());
        assert!(ExecutionGeneration::new(0).is_err());
        assert!(ExecutionGeneration::new(1).unwrap() < ExecutionGeneration::new(2).unwrap());
        assert_eq!(
            BlobDigest::from_bytes(b"abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(BlobDigest::parse("ABC").is_err());
    }
    #[test]
    fn spec_and_bounds_validate() {
        assert!(spec().validate().is_ok());
        let mut s = spec();
        s.command.argv.clear();
        assert!(s.validate().is_err());
        let mut s = spec();
        s.command.argv[0] = "x".repeat(MAX_ARG_BYTES + 1);
        assert!(s.validate().is_err());
    }
    #[test]
    fn paths_env_metadata_outputs_and_roundtrip() {
        assert!(RelativePath::new("/tmp").is_err());
        assert!(RelativePath::new("a/../b").is_err());
        assert!(EnvironmentEntry::new("1BAD", "x").is_err());
        assert!(EnvironmentEntry::new("OK", "a\0b").is_err());
        let s = spec();
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<ExecutionSpec>(&j).unwrap(), s);
    }

    #[test]
    fn execution_spec_debug_redacts_credentials_and_command_arguments() {
        let mut spec = spec();
        spec.command.argv = vec!["tool".into(), "--token=argv-secret".into()];
        spec.command.environment = vec![EnvironmentEntry::new("API_TOKEN", "env-secret").unwrap()];
        spec.command.stdin = StdinPolicy::Bytes(b"stdin-secret".to_vec());
        spec.metadata = vec![("authorization".into(), "metadata-secret".into())];
        let debug = format!("{spec:?}");
        for secret in [
            "argv-secret",
            "env-secret",
            "stdin-secret",
            "metadata-secret",
        ] {
            assert!(!debug.contains(secret), "Debug leaked {secret}");
        }
        assert!(debug.contains("[REDACTED]"));
        let event = ExecutionEvent {
            sequence: EventSequence::new(1),
            kind: ExecutionEventKind::Stdout(b"output-secret".to_vec()),
            metadata: EventMetadata {
                fields: vec![("credential".into(), "event-metadata-secret".into())],
            },
        };
        let debug = format!("{event:?}");
        assert!(!debug.contains("output-secret"));
        assert!(!debug.contains("event-metadata-secret"));
    }

    #[test]
    fn capability_bounds_validate() {
        let c = NodeCapabilities {
            protocol: ProtocolVersionRange {
                min: ProtocolVersion { major: 1, minor: 0 },
                max: ProtocolVersion { major: 1, minor: 2 },
            },
            features: vec!["exec.argv.v1".into()],
            max_active_executions: 4,
        };
        assert!(c.validate().is_ok());
        let mut invalid = c;
        invalid.protocol.min = ProtocolVersion { major: 2, minor: 0 };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn resource_requirements_preserve_legacy_best_effort_shape_and_accept_required_values() {
        let best_effort: Requirement<u64> =
            serde_json::from_value(serde_json::json!(4096)).unwrap();
        let required: Requirement<u64> =
            serde_json::from_value(serde_json::json!({"required": 8192})).unwrap();
        let missing: Requirement<u64> = serde_json::from_value(serde_json::Value::Null).unwrap();
        assert_eq!(best_effort, Requirement::BestEffort(4096));
        assert_eq!(required, Requirement::Required(8192));
        assert_eq!(missing, Requirement::NotRequested);
        assert_eq!(
            serde_json::to_value(best_effort).unwrap(),
            serde_json::json!(4096)
        );
        assert_eq!(
            serde_json::to_value(required).unwrap(),
            serde_json::json!({"required": 8192})
        );

        let requirements = ResourceRequirements {
            memory_bytes: Requirement::BestEffort(4096),
            cpu_millis: Requirement::Required(500),
            pids: Requirement::NotRequested,
        };
        assert!(requirements.validate().is_ok());
        assert!(
            ResourceRequirements {
                memory_bytes: Requirement::Required(0),
                cpu_millis: Requirement::NotRequested,
                pids: Requirement::NotRequested,
            }
            .validate()
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(ResourceRequirements {
                memory_bytes: Requirement::NotRequested,
                cpu_millis: Requirement::BestEffort(500),
                pids: Requirement::NotRequested,
            })
            .unwrap(),
            serde_json::json!({
                "memory_bytes": null,
                "cpu_millis": 500,
                "pids": null
            })
        );
    }

    #[test]
    fn canonical_request_digest_ignores_unordered_field_order() {
        let mut left = spec();
        left.command
            .environment
            .push(EnvironmentEntry::new("ZED", "z").unwrap());
        left.metadata = vec![("z".into(), "last".into()), ("a".into(), "first".into())];
        let mut right = left.clone();
        right.command.environment.reverse();
        right.metadata.reverse();
        assert_eq!(
            request_digest(&left).unwrap(),
            request_digest(&right).unwrap()
        );

        right.command.argv[1] = "different".into();
        assert_ne!(
            request_digest(&left).unwrap(),
            request_digest(&right).unwrap()
        );
    }

    #[test]
    fn workspace_identity_participates_in_versioned_execution_digest() {
        let spec = spec();
        let one = WorkspaceId::new("workspace-one").unwrap();
        let two = WorkspaceId::new("workspace-two").unwrap();
        let (version_one, digest_one) = request_digest_with_workspace(&spec, Some(&one)).unwrap();
        let (version_two, digest_two) = request_digest_with_workspace(&spec, Some(&two)).unwrap();
        let (version_none, digest_none) = request_digest_with_workspace(&spec, None).unwrap();
        assert_eq!(version_one, CANONICAL_WORKSPACE_REQUEST_VERSION);
        assert_eq!(version_two, version_one);
        assert_eq!(version_none, CANONICAL_REQUEST_VERSION);
        assert_ne!(digest_one, digest_two);
        assert_ne!(digest_one, digest_none);
    }

    #[test]
    fn workspace_manifest_validates_tree_and_has_order_independent_digest() {
        let directory = WorkspaceEntry::Directory {
            path: RelativePath::new("src").unwrap(),
        };
        let file = WorkspaceEntry::File {
            path: RelativePath::new("src/main.rs").unwrap(),
            digest: BlobDigest::from_bytes(b"main"),
            size_bytes: 4,
            executable: false,
        };
        let first = WorkspaceManifest {
            schema_version: 1,
            entries: vec![directory.clone(), file.clone()],
        };
        let second = WorkspaceManifest {
            schema_version: 1,
            entries: vec![file, directory],
        };
        assert!(first.validate().is_ok());
        assert_eq!(first.logical_bytes(), 4);
        assert_eq!(first.digest().unwrap(), second.digest().unwrap());
    }

    #[test]
    fn workspace_manifest_rejects_traversal_conflicts_and_symlinks() {
        let file = |path: &str| WorkspaceEntry::File {
            path: RelativePath::new(path).unwrap(),
            digest: BlobDigest::from_bytes(b"x"),
            size_bytes: 1,
            executable: false,
        };
        let manifest = |entries| WorkspaceManifest {
            schema_version: 1,
            entries,
        };
        assert!(validate_portable_workspace_path("../secret").is_err());
        assert!(validate_portable_workspace_path("C:/secret").is_err());
        assert!(validate_portable_workspace_path("a/CON.txt").is_err());
        assert!(validate_portable_workspace_path("café.txt").is_err());
        assert!(manifest(vec![file("a"), file("a/b")]).validate().is_err());
        assert!(manifest(vec![file("a"), file("a")]).validate().is_err());
        assert!(manifest(vec![file("A"), file("a")]).validate().is_err());
        assert!(
            manifest(vec![file("missing-parent/file")])
                .validate()
                .is_err()
        );
        assert!(
            manifest(vec![WorkspaceEntry::Symlink {
                path: RelativePath::new("link").unwrap(),
                target: "../../etc/passwd".into(),
            }])
            .validate()
            .is_err()
        );
    }

    #[test]
    fn workspace_manifest_enforces_depth_count_and_logical_size() {
        let entries = (0..=MAX_WORKSPACE_DEPTH)
            .map(|index| format!("d{index}"))
            .collect::<Vec<_>>();
        let deep = entries.join("/");
        assert!(validate_portable_workspace_path(&deep).is_err());
        let many = (0..=MAX_WORKSPACE_ENTRIES)
            .map(|index| WorkspaceEntry::Directory {
                path: RelativePath::new(format!("d{index}")).unwrap(),
            })
            .collect();
        assert!(
            WorkspaceManifest {
                schema_version: 1,
                entries: many,
            }
            .validate()
            .is_err()
        );
        let too_large = WorkspaceManifest {
            schema_version: 1,
            entries: vec![WorkspaceEntry::File {
                path: RelativePath::new("large.bin").unwrap(),
                digest: BlobDigest::from_bytes(b""),
                size_bytes: MAX_WORKSPACE_LOGICAL_BYTES + 1,
                executable: false,
            }],
        };
        assert!(too_large.validate().is_err());
    }

    #[test]
    fn duplicate_environment_names_are_rejected() {
        let mut value = spec();
        value
            .command
            .environment
            .push(EnvironmentEntry::new("LANG", "C").unwrap());
        assert!(value.validate().is_err());
    }
}
