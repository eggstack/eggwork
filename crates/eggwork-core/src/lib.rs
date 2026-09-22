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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentEntry {
    pub name: String,
    pub value: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StdinPolicy {
    Null,
    Bytes(Vec<u8>),
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
    pub memory_bytes: Option<u64>,
    pub cpu_millis: Option<u64>,
    pub pids: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Requirement<T> {
    NotRequested,
    BestEffort(T),
    Required(T),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSpec {
    pub schema_version: u16,
    pub command: CommandSpec,
    pub metadata: Vec<(String, String)>,
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
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMetadata {
    pub fields: Vec<(String, String)>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionEventKind {
    State(ExecutionState),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Diagnostic(String),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub schema_version: u16,
    pub code: String,
    pub message: String,
}

/// Stable canonical digest input: version prefix and serde_json output with struct field order.
pub fn request_digest(spec: &ExecutionSpec) -> Result<BlobDigest, serde_json::Error> {
    let mut bytes = b"eggwork-execution-spec\0v1\0".to_vec();
    bytes.extend(serde_json::to_vec(spec)?);
    Ok(BlobDigest::from_bytes(&bytes))
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
                    memory_bytes: None,
                    cpu_millis: None,
                    pids: None,
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
}
