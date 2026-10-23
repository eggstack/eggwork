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
/// Ceiling on the JSON encoding of a single [`ExecutionEvent`].
///
/// `Stdout`/`Stderr` carry `Vec<u8>` without a compact wire representation, so
/// `MAX_EVENT_CHUNK_BYTES` of payload serializes as a JSON number array at up
/// to four characters per byte plus the event envelope. A node refuses to
/// persist a larger event and a conforming client must still be able to read
/// one back, so both sides derive their bound from this constant rather than
/// from the payload cap.
pub const MAX_EVENT_BYTES: usize = 512 * 1024;
pub const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TIMEOUT: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const MAX_CAPABILITIES: usize = 256;
pub const MAX_WORKSPACE_ENTRIES: usize = 4096;
pub const MAX_WORKSPACE_PATH_BYTES: usize = 1024 * 1024;
pub const MAX_WORKSPACE_DEPTH: usize = 128;
pub const MAX_WORKSPACE_LOGICAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Prefix of every diagnostic payload.
///
/// A diagnostic is carried by [`ExecutionEventKind::Diagnostic`] as a single
/// bounded string. The grammar is
/// `eggwork.diagnostic.<class>.v1 key=value key=value …`, so the prefix and
/// the schema suffix bracket the class name and make the payload
/// self-describing: a reader can tell what kind of fact it is holding without
/// out-of-band knowledge.
pub const DIAGNOSTIC_PREFIX: &str = "eggwork.diagnostic.";

/// Schema version suffix of every diagnostic payload.
pub const DIAGNOSTIC_SCHEMA_SUFFIX: &str = ".v1";

/// The class of a diagnostic that reports a requested best-effort filesystem
/// isolation that was not applied.
pub const DIAGNOSTIC_CLASS_ISOLATION_DEGRADED: &str = "isolation.degraded";

/// The class of a diagnostic that reports a requested best-effort resource
/// enforcement that was not applied.
pub const DIAGNOSTIC_CLASS_RESOURCES_DEGRADED: &str = "resources.degraded";

/// The closed set of diagnostic classes. A payload naming anything else is
/// refused rather than guessed at, because a class is what a consumer
/// dispatches on.
pub const DIAGNOSTIC_CLASSES: [&str; 2] = [
    DIAGNOSTIC_CLASS_ISOLATION_DEGRADED,
    DIAGNOSTIC_CLASS_RESOURCES_DEGRADED,
];

/// Ceiling on a whole diagnostic payload.
///
/// The bound is deliberately the same as one event-metadata value: a
/// diagnostic is operator-facing text carried inside an event, so it must not
/// be able to dominate the event it rides in.
pub const MAX_DIAGNOSTIC_PAYLOAD_BYTES: usize = MAX_METADATA_VALUE_BYTES;

/// Ceiling on one diagnostic key.
pub const MAX_DIAGNOSTIC_KEY_BYTES: usize = 32;

/// The literal marker appended to a diagnostic value that had to be shortened.
///
/// The marker is what makes truncation *observable*: a reader can tell a
/// shortened reason from a complete one, so a bounded payload never silently
/// reads as a complete statement.
pub const DIAGNOSTIC_TRUNCATION_MARKER: &str = "...[truncated]";

/// Errors from building or parsing a diagnostic payload.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiagnosticError {
    #[error("diagnostic class {0} is not in the closed class set")]
    UnknownClass(String),
    #[error("diagnostic key {0} is not a valid diagnostic key")]
    InvalidKey(String),
    #[error("diagnostic key {0} is reserved")]
    ReservedKey(String),
    #[error("diagnostic payload does not match the documented grammar")]
    InvalidGrammar,
    #[error("diagnostic payload exceeds its {max}-byte limit")]
    TooLong { max: usize },
}

/// A bounded, machine-readable diagnostic payload.
///
/// The payload is built from a class drawn from [`DIAGNOSTIC_CLASSES`] and an
/// ordered list of key/value pairs, and is rendered by [`DiagnosticPayload::build`]
/// as `eggwork.diagnostic.<class>.v1 key=value key=value …`.
///
/// Two bounds are enforced here rather than at the call site, because a
/// diagnostic rides inside an event and a caller that forgets to bound it would
/// otherwise be able to overflow the event envelope:
///
/// * the assembled payload never exceeds [`MAX_DIAGNOSTIC_PAYLOAD_BYTES`];
/// * a value that does not fit is shortened on a UTF-8 character boundary and
///   gains [`DIAGNOSTIC_TRUNCATION_MARKER`], so the loss is visible in the
///   payload instead of being silent.
///
/// Values are normalised to printable ASCII (everything outside `!`..=`~`
/// becomes `_`) so that the space-separated `key=value` grammar stays
/// unambiguous: a value can never contain the separator or a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticPayload {
    class: String,
    fields: Vec<(String, String)>,
}

impl DiagnosticPayload {
    /// Creates a payload for a class in the closed set.
    ///
    /// A class outside [`DIAGNOSTIC_CLASSES`] is refused: the class is what a
    /// consumer dispatches on, so an unknown one cannot be honoured.
    pub fn new(class: &str) -> Result<Self, DiagnosticError> {
        if !DIAGNOSTIC_CLASSES.contains(&class) {
            return Err(DiagnosticError::UnknownClass(class.to_owned()));
        }
        Ok(Self {
            class: class.to_owned(),
            fields: Vec::new(),
        })
    }

    /// The diagnostic class.
    pub fn class(&self) -> &str {
        &self.class
    }

    /// Adds a key/value pair.
    ///
    /// The key must be a lowercase ASCII token of at most
    /// [`MAX_DIAGNOSTIC_KEY_BYTES`] bytes and must not be `class`, which the
    /// builder derives from the payload's own class.
    pub fn set(
        &mut self,
        key: &str,
        value: impl Into<String>,
    ) -> Result<&mut Self, DiagnosticError> {
        if key == "class" {
            return Err(DiagnosticError::ReservedKey(key.to_owned()));
        }
        if !valid_diagnostic_key(key) {
            return Err(DiagnosticError::InvalidKey(key.to_owned()));
        }
        let value = normalize_diagnostic_value(&value.into());
        self.fields.push((key.to_owned(), value));
        Ok(self)
    }

    /// Renders the bounded payload.
    ///
    /// The result is never longer than [`MAX_DIAGNOSTIC_PAYLOAD_BYTES`]. When
    /// the fields do not fit, values are shortened in insertion order: the
    /// first value that does not fit is cut on a UTF-8 character boundary and
    /// gains [`DIAGNOSTIC_TRUNCATION_MARKER`], and the fields after it are
    /// omitted. Insert the free-form `reason` last so that it absorbs the
    /// overage and the structured keys survive intact.
    pub fn build(&self) -> String {
        let prefix = format!(
            "{DIAGNOSTIC_PREFIX}{}{DIAGNOSTIC_SCHEMA_SUFFIX}",
            self.class
        );
        // The class is part of the prefix and of the key set, and it is drawn
        // from a closed set, so it is never shortened.
        let fixed = prefix.len() + 1 + "class=".len() + self.class.len();
        let mut budget = MAX_DIAGNOSTIC_PAYLOAD_BYTES.saturating_sub(fixed);
        let mut rendered: Vec<String> = Vec::with_capacity(self.fields.len());
        for (key, value) in &self.fields {
            let overhead = 1 + key.len() + 1;
            let available = budget.saturating_sub(overhead);
            if value.len() <= available {
                rendered.push(format!("{key}={value}"));
                budget -= overhead + value.len();
                continue;
            }
            let marker = DIAGNOSTIC_TRUNCATION_MARKER;
            if overhead + marker.len() > budget {
                // Not even the marker fits: the field is omitted rather than
                // pushed past the bound.
                continue;
            }
            let keep = available - marker.len();
            rendered.push(format!("{key}={}{marker}", truncate_utf8(value, keep)));
            budget -= overhead + keep + marker.len();
        }
        let mut payload = prefix;
        payload.push_str(" class=");
        payload.push_str(&self.class);
        for part in rendered {
            payload.push(' ');
            payload.push_str(&part);
        }
        payload
    }

    /// Parses a payload back into its class and fields.
    ///
    /// This is the validator a consumer uses to decide whether a diagnostic it
    /// received is well formed. It refuses a payload that does not match the
    /// documented grammar, names a class outside the closed set, exceeds the
    /// bound, or omits one of the required keys.
    pub fn parse(payload: &str) -> Result<Self, DiagnosticError> {
        if payload.len() > MAX_DIAGNOSTIC_PAYLOAD_BYTES {
            return Err(DiagnosticError::TooLong {
                max: MAX_DIAGNOSTIC_PAYLOAD_BYTES,
            });
        }
        let rest = payload
            .strip_prefix(DIAGNOSTIC_PREFIX)
            .ok_or(DiagnosticError::InvalidGrammar)?;
        let (class, rest) = rest
            .split_once(DIAGNOSTIC_SCHEMA_SUFFIX)
            .ok_or(DiagnosticError::InvalidGrammar)?;
        if !DIAGNOSTIC_CLASSES.contains(&class) {
            return Err(DiagnosticError::UnknownClass(class.to_owned()));
        }
        let mut parsed = Self::new(class)?;
        for token in rest.split_whitespace() {
            let (key, value) = token
                .split_once('=')
                .ok_or(DiagnosticError::InvalidGrammar)?;
            if key == "class" {
                if value != class {
                    return Err(DiagnosticError::InvalidGrammar);
                }
                continue;
            }
            if !valid_diagnostic_key(key) {
                return Err(DiagnosticError::InvalidKey(key.to_owned()));
            }
            parsed.fields.push((key.to_owned(), value.to_owned()));
        }
        // `class` is implied by the prefix; the other two are the minimum key
        // set every diagnostic has to carry, so a payload without them is not
        // a diagnostic this grammar can describe.
        for required in ["requirement", "reason"] {
            if !parsed.fields.iter().any(|(key, _)| key == required) {
                return Err(DiagnosticError::InvalidGrammar);
            }
        }
        Ok(parsed)
    }
}

/// A diagnostic key is a lowercase ASCII token.
fn valid_diagnostic_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_DIAGNOSTIC_KEY_BYTES
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Normalises a diagnostic value to printable ASCII.
///
/// The grammar separates fields with spaces, so a value that could contain a
/// space would make the payload ambiguous. Every character outside `!`..=`~`
/// becomes `_`, which keeps the value a single token without inventing an
/// escaping scheme a reader would have to know about.
fn normalize_diagnostic_value(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if ('!'..='~').contains(&character) {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Cuts a string to at most `max_bytes`, on a UTF-8 character boundary.
fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

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

/// Bounded deterministic patch applied to a retained canonical manifest.
///
/// The patch names its content base by digest and carries exact-path
/// remove/upsert entries. Applying a patch to a base manifest MUST yield the
/// same canonical manifest (and digest) as submitting the final manifest
/// directly. Patch ordering never affects the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifestPatch {
    pub schema_version: u16,
    pub base_manifest_digest: BlobDigest,
    pub entries: Vec<WorkspacePatchEntry>,
}

/// One exact-path patch operation. `Remove` deletes exactly that path;
/// removing a directory requires explicit descendant removals (no recursive
/// delete). `Directory`/`File` upsert exactly that path. Rename is
/// remove + upsert. Symlinks remain unsupported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspacePatchEntry {
    Remove {
        path: RelativePath,
    },
    Directory {
        path: RelativePath,
    },
    File {
        path: RelativePath,
        digest: BlobDigest,
        size_bytes: u64,
        executable: bool,
    },
}

impl WorkspacePatchEntry {
    pub fn path(&self) -> &RelativePath {
        match self {
            Self::Remove { path } | Self::Directory { path } | Self::File { path, .. } => path,
        }
    }
}

impl WorkspaceManifestPatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != 1 {
            return Err(ValidationError::OutOfRange {
                field: "workspace patch schema version",
            });
        }
        if self.entries.len() > MAX_WORKSPACE_ENTRIES {
            return Err(ValidationError::TooMany {
                field: "workspace patch entries",
                max: MAX_WORKSPACE_ENTRIES,
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for entry in &self.entries {
            if !seen.insert(entry.path().as_str()) {
                return Err(ValidationError::InvalidSyntax {
                    field: "duplicate workspace patch path",
                });
            }
        }
        Ok(())
    }

    /// Apply this patch to a base manifest through a deterministic path map,
    /// then validate the final manifest with the existing validator. No
    /// second path validator is introduced: portable path, case-collision,
    /// parent-directory, symlink, depth, count, and size rules all come from
    /// [`WorkspaceManifest::validate`].
    pub fn apply_to(&self, base: &WorkspaceManifest) -> Result<WorkspaceManifest, ValidationError> {
        self.validate()?;
        let mut tree = std::collections::BTreeMap::<String, WorkspaceEntry>::new();
        for entry in &base.entries {
            tree.insert(entry.path().as_str().to_owned(), entry.clone());
        }
        for entry in &self.entries {
            match entry {
                WorkspacePatchEntry::Remove { path } => {
                    tree.remove(path.as_str());
                }
                WorkspacePatchEntry::Directory { path } => {
                    tree.insert(
                        path.as_str().to_owned(),
                        WorkspaceEntry::Directory { path: path.clone() },
                    );
                }
                WorkspacePatchEntry::File {
                    path,
                    digest,
                    size_bytes,
                    executable,
                } => {
                    tree.insert(
                        path.as_str().to_owned(),
                        WorkspaceEntry::File {
                            path: path.clone(),
                            digest: digest.clone(),
                            size_bytes: *size_bytes,
                            executable: *executable,
                        },
                    );
                }
            }
        }
        let final_manifest = WorkspaceManifest {
            schema_version: 1,
            entries: tree.into_values().collect(),
        };
        final_manifest.validate()?;
        Ok(final_manifest)
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

// ---------------------------------------------------------------------------
// Optional host telemetry
// ---------------------------------------------------------------------------
//
// These types are the *normalized* vocabulary for host-wide measurements. They
// are deliberately protocol-neutral: no provider's wire types, field names or
// collection shapes appear here, and nothing in this module knows how a
// measurement was obtained. A provider-specific payload is validated and
// translated into these types before it is cached, so the vocabulary can be
// projected onto a wire contract without dragging a provider's schema along.
//
// Host telemetry is advisory observation, never enforcement evidence. Nothing
// in this module grants, withholds, or qualifies an execution capability, and
// `NodeStatus`/`NodeCapabilities` keep their existing meaning.

/// Ceiling on the accepted sampling cadence of a host observation.
///
/// Mirrors the bound the provider protocol applies, so a payload that the
/// provider itself would reject is rejected here too.
pub const MAX_TELEMETRY_SAMPLE_INTERVAL_MS: u64 = 86_400_000;

/// Where a host-wide observation came from.
///
/// The set is closed on purpose: an unrecognized source is a protocol error
/// for a reader, not a new variant it may silently accept. Adding a source
/// means adding a variant here and re-freezing the wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetrySource {
    /// Readings come from a locally installed Gregg daemon.
    Gregg,
}

/// Lifecycle state of the optional host-telemetry provider.
///
/// The state describes the *provider*, not the host. A provider that is
/// `Disabled` is not a host with zero usage; it is a host with no provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryProviderState {
    /// No provider is configured. Nothing is polled and nothing is claimed.
    Disabled,
    /// A provider is configured but has not produced an observation yet.
    Warming,
    /// A validated observation is available and within its freshness bound.
    Ready,
    /// An observation is available but older than the freshness bound.
    Stale,
    /// The provider could not be reached, or answered with something unusable.
    Unavailable,
    /// The provider answered with a payload that failed validation.
    Invalid,
}

/// Why the last provider interaction failed.
///
/// Categories are a closed, bounded set and carry no provider-supplied text, so
/// a failure can be reported without leaking a response body, a URL, or a host
/// path into a response, a log line, or a metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryErrorCategory {
    /// The connection could not be established.
    Connection,
    /// The request exceeded its deadline.
    Timeout,
    /// The provider answered with an unexpected HTTP status.
    HttpStatus,
    /// The response body was not the expected JSON document.
    Malformed,
    /// The response carried an unsupported schema version.
    SchemaVersion,
    /// The response parsed but violated the protocol's invariants.
    Invalid,
    /// The response exceeded the accepted body size.
    TooLarge,
    /// The observation timestamp is outside the accepted clock-skew window.
    ClockSkew,
}

/// One host-wide measurement, together with the reason it is absent.
///
/// A missing measurement is never zero. `Unsupported` means the host platform
/// cannot produce the metric at all; `Unknown` means it can but this sample
/// carried no value; `Value` carries a validated measurement. A reader can
/// therefore tell "this host does not have swap" apart from "this host has
/// swap and it is currently unused".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryMetric<T> {
    /// The host platform does not support this measurement.
    Unsupported,
    /// The measurement is supported, but this sample carried no value.
    Unknown,
    /// A validated measurement.
    Value(T),
}

impl<T> TelemetryMetric<T> {
    /// The measured value, or `None` when the metric is absent.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Unsupported | Self::Unknown => None,
        }
    }

    /// Whether the host platform does not support this measurement.
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported)
    }

    /// Whether the measurement is supported but was not reported.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

/// CPU utilization for the host as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryCpu {
    /// Logical CPU cores visible to the kernel.
    pub logical_cores: u32,
    /// Aggregate CPU busy percentage, `0.0..=100.0`.
    pub usage_pct: f32,
    /// Aggregate CPU I/O-wait percentage, `0.0..=100.0`.
    ///
    /// `None` when the host platform does not report I/O wait. It is never
    /// `Some(0.0)` as a substitute for "not reported".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iowait_pct: Option<f32>,
}

/// Physical memory utilization for the host as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryMemory {
    /// Bytes available without swapping.
    pub available_bytes: u64,
    /// Total physical memory in bytes.
    pub total_bytes: u64,
    /// Bytes currently resident in physical memory.
    pub used_bytes: u64,
}

/// Load averages for the host as a whole.
///
/// These are load averages, not percentages: they are unbounded above and are
/// only required to be finite and non-negative.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryLoad {
    /// One-minute load average.
    pub one: f32,
    /// Five-minute load average.
    pub five: f32,
    /// Fifteen-minute load average.
    pub fifteen: f32,
}

/// Swap utilization for the host as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetrySwap {
    /// Bytes currently in swap.
    pub used_bytes: u64,
    /// Total swap in bytes.
    pub total_bytes: u64,
}

/// Storage capacity for the host as a whole.
///
/// The figures are aggregates over every volume the provider reported. They
/// are not a per-volume claim, and they carry no mount path, device
/// identifier, or filesystem label.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryDisk {
    /// Free bytes across the reported volumes.
    pub available_bytes: u64,
    /// Total bytes across the reported volumes.
    pub total_bytes: u64,
    /// How many volumes contributed to the aggregate.
    pub volume_count: u32,
}

/// Aggregate disk throughput for the host as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryDiskIo {
    /// Aggregate read throughput in bytes per second.
    pub read_bytes_per_sec: u64,
    /// Aggregate write throughput in bytes per second.
    pub write_bytes_per_sec: u64,
}

/// Aggregate network throughput for the host as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryNetwork {
    /// Aggregate receive throughput in bytes per second.
    pub rx_bytes_per_sec: u64,
    /// Aggregate transmit throughput in bytes per second.
    pub tx_bytes_per_sec: u64,
}

/// One validated host-wide observation.
///
/// The observation is a *measurement*, not a verdict: it says what the host
/// looked like at `observed_at_unix_ms`, and it never says whether an
/// execution may run. Per-execution utilization is not representable here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostTelemetry {
    /// Provider wall-clock timestamp of the sample, in Unix milliseconds.
    pub observed_at_unix_ms: u64,
    /// Provider sampling cadence in milliseconds.
    pub sample_interval_ms: u64,
    /// CPU utilization.
    pub cpu: TelemetryMetric<TelemetryCpu>,
    /// Physical memory utilization.
    pub memory: TelemetryMetric<TelemetryMemory>,
    /// Load averages.
    pub load: TelemetryMetric<TelemetryLoad>,
    /// Swap utilization.
    pub swap: TelemetryMetric<TelemetrySwap>,
    /// Storage capacity.
    pub disk: TelemetryMetric<TelemetryDisk>,
    /// Aggregate disk throughput.
    pub disk_io: TelemetryMetric<TelemetryDiskIo>,
    /// Aggregate network throughput.
    pub network: TelemetryMetric<TelemetryNetwork>,
}

/// Wire schema version of the node telemetry response.
///
/// The contract is additive within version 1: a reader accepts a version-1
/// payload and refuses a version it does not know, rather than guessing.
pub const NODE_TELEMETRY_SCHEMA_VERSION: u16 = 1;

/// The node's host-wide telemetry observation, as served to an authorized caller.
///
/// This is the frozen wire contract for the node telemetry response. It is
/// additive and deliberately separate from
/// [`NodeStatus`]: `NodeStatus` answers "is this node accepting work", and
/// this answers "what did this host look like". The two are never merged, and
/// nothing here qualifies an execution.
///
/// The fields mean:
///
/// * `schema_version` — the contract version. A reader refuses anything else.
/// * `node_id` — the node that produced the observation, so a reader can tell
///   that a response came from the node it asked.
/// * `source` — where the measurement came from. The set is closed: a reader
///   refuses a source it does not know. The source names the *kind* of
///   provider; whether that provider is live is `state`'s answer, not this
///   field's.
/// * `state` — what the provider is doing, not what the host is doing. A node
///   with no provider is `disabled`, which is not the same as a host with no
///   usage.
/// * `age_ms` — how long ago, in milliseconds, the node received the
///   observation, measured on the node's monotonic clock. `None` when there is
///   no observation.
/// * `last_error` — the bounded category of the most recent provider failure.
///   `None` when the last interaction succeeded or none has failed.
/// * `observation` — the most recent measurement the node holds, or `None` when
///   it holds none.
///
/// The `state` and `observation` pair is the part a reader must understand:
///
/// | `state` | `observation` | meaning |
/// |---|---|---|
/// | `disabled` | absent | no provider is configured |
/// | `warming` | absent | the provider is alive but has not produced a sample |
/// | `ready` | present | a sample is present and within its freshness bound |
/// | `stale` | present | a sample is present but older than the freshness bound |
/// | `unavailable` | present or absent | the provider could not be reached |
/// | `invalid` | present or absent | the provider answered with something unusable |
///
/// A sample is *current* only when the state is `ready`. A reader that needs a
/// fresh measurement must check the state, not merely the presence of an
/// observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeTelemetry {
    pub schema_version: u16,
    pub node_id: NodeId,
    pub source: TelemetrySource,
    pub state: TelemetryProviderState,
    pub age_ms: Option<u64>,
    pub last_error: Option<TelemetryErrorCategory>,
    pub observation: Option<HostTelemetry>,
}

impl NodeTelemetry {
    /// Check the invariants of a telemetry response.
    ///
    /// The node applies this to what it serves, and a client applies it again
    /// to what it receives, so a node that skipped it cannot hand a reader a
    /// response the reader would have to reject.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != NODE_TELEMETRY_SCHEMA_VERSION {
            return Err(ValidationError::OutOfRange {
                field: "node telemetry schema version",
            });
        }
        match self.state {
            // A ready or stale node always holds the sample it is reporting.
            TelemetryProviderState::Ready | TelemetryProviderState::Stale => {
                if self.observation.is_none() {
                    return Err(ValidationError::OutOfRange {
                        field: "node telemetry observation",
                    });
                }
            }
            // A node with no provider, or one that has not sampled yet, holds
            // no sample. Reporting one would claim a measurement the node
            // never took.
            TelemetryProviderState::Disabled | TelemetryProviderState::Warming => {
                if self.observation.is_some() {
                    return Err(ValidationError::OutOfRange {
                        field: "node telemetry observation",
                    });
                }
            }
            // A failed provider may still be holding the last sample it took.
            TelemetryProviderState::Unavailable | TelemetryProviderState::Invalid => {}
        }
        if self.observation.is_some() != self.age_ms.is_some() {
            return Err(ValidationError::OutOfRange {
                field: "node telemetry age",
            });
        }
        if let Some(observation) = &self.observation {
            observation.validate()?;
        }
        Ok(())
    }
}

/// Reject a percentage that is not a finite value in `0.0..=100.0`.
///
/// A non-finite percentage cannot be serialized as a JSON number, and an
/// out-of-range one is a provider defect rather than a measurement. Both are
/// refused instead of being clamped, because a clamped value is a fabricated
/// measurement.
fn validate_percentage(value: f32, field: &'static str) -> Result<(), ValidationError> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(ValidationError::OutOfRange { field });
    }
    Ok(())
}

/// Reject a load average that is not a finite, non-negative value.
fn validate_load(value: f32, field: &'static str) -> Result<(), ValidationError> {
    if !value.is_finite() || value < 0.0 {
        return Err(ValidationError::OutOfRange { field });
    }
    Ok(())
}

impl HostTelemetry {
    /// Check the invariants every consumer of an observation depends on.
    ///
    /// This is the same check on both sides of the boundary: the node applies
    /// it before caching a provider payload, and a client applies it again
    /// after decoding, so a node that skips it cannot hand a reader a
    /// measurement the reader would have to reject.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.observed_at_unix_ms == 0 {
            return Err(ValidationError::OutOfRange {
                field: "telemetry observation timestamp",
            });
        }
        if self.sample_interval_ms == 0
            || self.sample_interval_ms > MAX_TELEMETRY_SAMPLE_INTERVAL_MS
        {
            return Err(ValidationError::OutOfRange {
                field: "telemetry sample interval",
            });
        }
        if let TelemetryMetric::Value(cpu) = &self.cpu {
            if cpu.logical_cores == 0 {
                return Err(ValidationError::OutOfRange {
                    field: "telemetry cpu logical cores",
                });
            }
            validate_percentage(cpu.usage_pct, "telemetry cpu usage")?;
            if let Some(iowait) = cpu.iowait_pct {
                validate_percentage(iowait, "telemetry cpu iowait")?;
            }
        }
        if let TelemetryMetric::Value(memory) = &self.memory
            && memory.used_bytes > memory.total_bytes
        {
            return Err(ValidationError::OutOfRange {
                field: "telemetry memory used bytes",
            });
        }
        if let TelemetryMetric::Value(load) = &self.load {
            validate_load(load.one, "telemetry load one")?;
            validate_load(load.five, "telemetry load five")?;
            validate_load(load.fifteen, "telemetry load fifteen")?;
        }
        if let TelemetryMetric::Value(swap) = &self.swap
            && swap.used_bytes > swap.total_bytes
        {
            return Err(ValidationError::OutOfRange {
                field: "telemetry swap used bytes",
            });
        }
        Ok(())
    }
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
    /// A diagnostic payload matches the documented grammar, carries the class
    /// and the required keys, and round-trips through the parser.
    #[test]
    fn a_diagnostic_payload_matches_the_documented_grammar() {
        let mut payload = DiagnosticPayload::new(DIAGNOSTIC_CLASS_ISOLATION_DEGRADED).unwrap();
        payload.set("requirement", "best_effort").unwrap();
        payload.set("profile", "workspace_rw").unwrap();
        payload
            .set("reason", "sandbox helper trust check failed")
            .unwrap();
        let built = payload.build();
        assert!(built.starts_with("eggwork.diagnostic.isolation.degraded.v1 "));
        assert!(built.contains(" class=isolation.degraded"));
        assert!(built.contains(" requirement=best_effort"));
        assert!(built.contains(" profile=workspace_rw"));
        assert!(built.contains(" reason=sandbox_helper_trust_check_failed"));
        assert!(built.len() <= MAX_DIAGNOSTIC_PAYLOAD_BYTES);
        let parsed = DiagnosticPayload::parse(&built).unwrap();
        assert_eq!(parsed.class(), DIAGNOSTIC_CLASS_ISOLATION_DEGRADED);
    }
    /// The bound is enforced, and an over-long reason is truncated in a way a
    /// reader can see rather than silently.
    #[test]
    fn an_over_long_diagnostic_reason_is_truncated_observably() {
        let mut payload = DiagnosticPayload::new(DIAGNOSTIC_CLASS_RESOURCES_DEGRADED).unwrap();
        payload.set("requirement", "best_effort").unwrap();
        payload.set("dimension", "memory").unwrap();
        payload
            .set("reason", "r".repeat(MAX_DIAGNOSTIC_PAYLOAD_BYTES * 2))
            .unwrap();
        let built = payload.build();
        assert!(built.len() <= MAX_DIAGNOSTIC_PAYLOAD_BYTES);
        assert!(built.contains(DIAGNOSTIC_TRUNCATION_MARKER));
        // The structured keys survive the truncation; only the free-form
        // reason is shortened.
        assert!(built.contains(" requirement=best_effort"));
        assert!(built.contains(" dimension=memory"));
        let reason = built
            .split_whitespace()
            .find(|token| token.starts_with("reason="))
            .unwrap();
        assert!(reason.ends_with(DIAGNOSTIC_TRUNCATION_MARKER));
    }
    /// A value that cannot be shortened without losing its meaning still
    /// yields a bounded payload, and the bound holds for a multi-byte value.
    #[test]
    fn a_diagnostic_payload_stays_within_its_bound() {
        let mut payload = DiagnosticPayload::new(DIAGNOSTIC_CLASS_ISOLATION_DEGRADED).unwrap();
        payload.set("requirement", "best_effort").unwrap();
        payload
            .set("reason", "é".repeat(MAX_DIAGNOSTIC_PAYLOAD_BYTES))
            .unwrap();
        let built = payload.build();
        assert!(built.len() <= MAX_DIAGNOSTIC_PAYLOAD_BYTES);
        assert!(built.contains(DIAGNOSTIC_TRUNCATION_MARKER));
        assert!(DiagnosticPayload::parse(&built).is_ok());
    }
    /// The parser refuses what the grammar does not allow: an unknown class, a
    /// malformed token, an over-long payload, and a missing required key.
    #[test]
    fn a_malformed_diagnostic_payload_is_refused() {
        assert!(DiagnosticPayload::parse("eggwork.diagnostic.nope.v1 class=nope").is_err());
        assert!(
            DiagnosticPayload::parse(
                "eggwork.diagnostic.isolation.degraded.v1 class=isolation.degraded"
            )
            .is_err()
        );
        assert!(DiagnosticPayload::parse("not a diagnostic").is_err());
        assert!(DiagnosticPayload::parse(&"x".repeat(MAX_DIAGNOSTIC_PAYLOAD_BYTES + 1)).is_err());
        assert!(
            DiagnosticPayload::parse(
                "eggwork.diagnostic.isolation.degraded.v1 class=isolation.degraded requirement=best_effort"
            )
            .is_err()
        );
        assert!(DiagnosticPayload::new("isolation.degraded.other").is_err());
    }
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

    fn patch_base_manifest() -> WorkspaceManifest {
        WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                WorkspaceEntry::Directory {
                    path: RelativePath::new("src").unwrap(),
                },
                WorkspaceEntry::File {
                    path: RelativePath::new("src/main.rs").unwrap(),
                    digest: BlobDigest::from_bytes(b"v1"),
                    size_bytes: 2,
                    executable: false,
                },
                WorkspaceEntry::File {
                    path: RelativePath::new("old.txt").unwrap(),
                    digest: BlobDigest::from_bytes(b"old"),
                    size_bytes: 3,
                    executable: false,
                },
            ],
        }
    }

    fn patch_for(
        base_digest: BlobDigest,
        entries: Vec<WorkspacePatchEntry>,
    ) -> WorkspaceManifestPatch {
        WorkspaceManifestPatch {
            schema_version: 1,
            base_manifest_digest: base_digest,
            entries,
        }
    }

    #[test]
    fn workspace_patch_validates_schema_duplicates_and_bounds() {
        let base = patch_base_manifest().digest().unwrap();
        assert!(
            patch_for(
                base.clone(),
                vec![WorkspacePatchEntry::Directory {
                    path: RelativePath::new("extra").unwrap(),
                }],
            )
            .validate()
            .is_ok()
        );
        let mut invalid = patch_for(base.clone(), vec![]);
        invalid.schema_version = 2;
        assert!(invalid.validate().is_err());
        let duplicate = patch_for(
            base.clone(),
            vec![
                WorkspacePatchEntry::Remove {
                    path: RelativePath::new("old.txt").unwrap(),
                },
                WorkspacePatchEntry::Directory {
                    path: RelativePath::new("old.txt").unwrap(),
                },
            ],
        );
        assert!(duplicate.validate().is_err());
        let many = (0..=MAX_WORKSPACE_ENTRIES)
            .map(|index| WorkspacePatchEntry::Directory {
                path: RelativePath::new(format!("d{index}")).unwrap(),
            })
            .collect();
        assert!(patch_for(base, many).validate().is_err());
    }

    #[test]
    fn workspace_patch_apply_is_order_independent_and_digest_equivalent() {
        let base = patch_base_manifest();
        let base_digest = base.digest().unwrap();
        let new_digest = BlobDigest::from_bytes(b"v2");
        let entries = vec![
            WorkspacePatchEntry::Remove {
                path: RelativePath::new("old.txt").unwrap(),
            },
            WorkspacePatchEntry::File {
                path: RelativePath::new("src/main.rs").unwrap(),
                digest: new_digest.clone(),
                size_bytes: 2,
                executable: true,
            },
            WorkspacePatchEntry::File {
                path: RelativePath::new("added.txt").unwrap(),
                digest: BlobDigest::from_bytes(b"added"),
                size_bytes: 5,
                executable: false,
            },
        ];
        let mut reversed = entries.clone();
        reversed.reverse();
        let first = patch_for(base_digest.clone(), entries)
            .apply_to(&base)
            .unwrap();
        let second = patch_for(base_digest, reversed).apply_to(&base).unwrap();
        assert_eq!(first, second);
        let expected = WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                WorkspaceEntry::Directory {
                    path: RelativePath::new("src").unwrap(),
                },
                WorkspaceEntry::File {
                    path: RelativePath::new("src/main.rs").unwrap(),
                    digest: new_digest,
                    size_bytes: 2,
                    executable: true,
                },
                WorkspaceEntry::File {
                    path: RelativePath::new("added.txt").unwrap(),
                    digest: BlobDigest::from_bytes(b"added"),
                    size_bytes: 5,
                    executable: false,
                },
            ],
        };
        assert!(expected.validate().is_ok());
        assert_eq!(first.digest().unwrap(), expected.digest().unwrap());
    }

    #[test]
    fn workspace_patch_rejects_final_trees_the_full_path_would_reject() {
        let base = patch_base_manifest();
        let base_digest = base.digest().unwrap();
        // Missing parent directory in the final tree.
        let orphan = patch_for(
            base_digest.clone(),
            vec![WorkspacePatchEntry::File {
                path: RelativePath::new("nodir/file.txt").unwrap(),
                digest: BlobDigest::from_bytes(b"x"),
                size_bytes: 1,
                executable: false,
            }],
        );
        assert!(orphan.apply_to(&base).is_err());
        // Removing the parent of a retained child breaks parent validation.
        let orphaned_child = patch_for(
            base_digest.clone(),
            vec![WorkspacePatchEntry::Remove {
                path: RelativePath::new("src").unwrap(),
            }],
        );
        assert!(orphaned_child.apply_to(&base).is_err());
        // Case collision in the final tree.
        let collision = patch_for(
            base_digest.clone(),
            vec![WorkspacePatchEntry::File {
                path: RelativePath::new("SRC/MAIN.RS").unwrap(),
                digest: BlobDigest::from_bytes(b"x"),
                size_bytes: 1,
                executable: false,
            }],
        );
        assert!(collision.apply_to(&base).is_err());
        // A patch cannot smuggle a symlink past the shared validator.
        let symlink_like = WorkspaceManifest {
            schema_version: 1,
            entries: vec![WorkspaceEntry::Symlink {
                path: RelativePath::new("link").unwrap(),
                target: "../../etc/passwd".into(),
            }],
        };
        assert!(symlink_like.validate().is_err());
        // Removing a path that is already absent is a deterministic no-op.
        let noop = patch_for(
            base_digest,
            vec![WorkspacePatchEntry::Remove {
                path: RelativePath::new("absent.txt").unwrap(),
            }],
        )
        .apply_to(&base)
        .unwrap();
        assert_eq!(noop.digest().unwrap(), base.digest().unwrap());
    }

    fn observation() -> HostTelemetry {
        HostTelemetry {
            observed_at_unix_ms: 1_700_000_000_000,
            sample_interval_ms: 1_000,
            cpu: TelemetryMetric::Value(TelemetryCpu {
                logical_cores: 8,
                usage_pct: 12.5,
                iowait_pct: Some(0.5),
            }),
            memory: TelemetryMetric::Value(TelemetryMemory {
                available_bytes: 4_000,
                total_bytes: 8_000,
                used_bytes: 4_000,
            }),
            load: TelemetryMetric::Unsupported,
            swap: TelemetryMetric::Unknown,
            disk: TelemetryMetric::Value(TelemetryDisk {
                available_bytes: 100,
                total_bytes: 500,
                volume_count: 2,
            }),
            disk_io: TelemetryMetric::Value(TelemetryDiskIo {
                read_bytes_per_sec: 10,
                write_bytes_per_sec: 20,
            }),
            network: TelemetryMetric::Value(TelemetryNetwork {
                rx_bytes_per_sec: 30,
                tx_bytes_per_sec: 40,
            }),
        }
    }

    #[test]
    fn a_valid_observation_validates() {
        assert!(observation().validate().is_ok());
    }

    #[test]
    fn a_missing_measurement_is_not_zero() {
        let sample = observation();
        assert!(sample.load.is_unsupported());
        assert!(sample.swap.is_unknown());
        assert!(sample.load.value().is_none());
        assert!(sample.swap.value().is_none());
        assert_eq!(sample.cpu.value().map(|cpu| cpu.logical_cores), Some(8));
    }

    #[test]
    fn out_of_range_and_non_finite_measurements_are_refused() {
        let mut sample = observation();
        if let TelemetryMetric::Value(cpu) = &mut sample.cpu {
            cpu.usage_pct = 100.5;
        }
        assert!(sample.validate().is_err());

        let mut sample = observation();
        if let TelemetryMetric::Value(cpu) = &mut sample.cpu {
            cpu.usage_pct = f32::NAN;
        }
        assert!(sample.validate().is_err());

        let mut sample = observation();
        if let TelemetryMetric::Value(cpu) = &mut sample.cpu {
            cpu.logical_cores = 0;
        }
        assert!(sample.validate().is_err());

        let mut sample = observation();
        if let TelemetryMetric::Value(memory) = &mut sample.memory {
            memory.used_bytes = memory.total_bytes + 1;
        }
        assert!(sample.validate().is_err());

        let mut sample = observation();
        sample.swap = TelemetryMetric::Value(TelemetrySwap {
            used_bytes: 1,
            total_bytes: 0,
        });
        assert!(sample.validate().is_err());

        let mut sample = observation();
        sample.sample_interval_ms = 0;
        assert!(sample.validate().is_err());

        let mut sample = observation();
        sample.sample_interval_ms = MAX_TELEMETRY_SAMPLE_INTERVAL_MS + 1;
        assert!(sample.validate().is_err());

        let mut sample = observation();
        sample.observed_at_unix_ms = 0;
        assert!(sample.validate().is_err());
    }

    #[test]
    fn a_load_average_may_exceed_one_hundred() {
        let mut sample = observation();
        sample.load = TelemetryMetric::Value(TelemetryLoad {
            one: 12.5,
            five: 8.0,
            fifteen: 0.25,
        });
        assert!(sample.validate().is_ok());

        sample.load = TelemetryMetric::Value(TelemetryLoad {
            one: f32::INFINITY,
            five: 0.0,
            fifteen: 0.0,
        });
        assert!(sample.validate().is_err());
    }

    #[test]
    fn the_observation_round_trips_through_json() {
        let encoded = serde_json::to_string(&observation()).unwrap();
        let decoded: HostTelemetry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, observation());
        // An absent measurement keeps its reason instead of decaying into zero.
        assert!(encoded.contains("\"unsupported\""));
        assert!(encoded.contains("\"unknown\""));
    }

    #[test]
    fn an_observation_with_an_unknown_field_is_refused() {
        let mut value = serde_json::to_value(observation()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), serde_json::json!(1));
        assert!(serde_json::from_value::<HostTelemetry>(value).is_err());
    }

    fn node_telemetry() -> NodeTelemetry {
        NodeTelemetry {
            schema_version: NODE_TELEMETRY_SCHEMA_VERSION,
            node_id: NodeId::new("node-a").unwrap(),
            source: TelemetrySource::Gregg,
            state: TelemetryProviderState::Ready,
            age_ms: Some(12),
            last_error: None,
            observation: Some(observation()),
        }
    }

    #[test]
    fn a_ready_telemetry_response_validates() {
        assert!(node_telemetry().validate().is_ok());
    }

    #[test]
    fn a_telemetry_response_with_an_unknown_schema_version_is_refused() {
        let mut telemetry = node_telemetry();
        telemetry.schema_version = NODE_TELEMETRY_SCHEMA_VERSION + 1;
        assert!(telemetry.validate().is_err());
    }

    #[test]
    fn a_ready_telemetry_response_without_an_observation_is_refused() {
        let mut telemetry = node_telemetry();
        telemetry.observation = None;
        assert!(telemetry.validate().is_err());
    }

    #[test]
    fn a_disabled_telemetry_response_with_an_observation_is_refused() {
        let mut telemetry = node_telemetry();
        telemetry.state = TelemetryProviderState::Disabled;
        assert!(telemetry.validate().is_err());
    }

    #[test]
    fn a_disabled_telemetry_response_is_valid_without_an_observation() {
        let mut telemetry = node_telemetry();
        telemetry.state = TelemetryProviderState::Disabled;
        telemetry.observation = None;
        telemetry.age_ms = None;
        assert!(telemetry.validate().is_ok());
    }

    #[test]
    fn a_telemetry_response_whose_age_contradicts_its_observation_is_refused() {
        let mut telemetry = node_telemetry();
        telemetry.age_ms = None;
        assert!(telemetry.validate().is_err());
    }

    #[test]
    fn a_telemetry_response_round_trips_through_json() {
        let encoded = serde_json::to_string(&node_telemetry()).unwrap();
        let decoded: NodeTelemetry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, node_telemetry());
        let mut value = serde_json::to_value(node_telemetry()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), serde_json::json!(1));
        assert!(serde_json::from_value::<NodeTelemetry>(value).is_err());
    }
}
