//! Optional, default-off host telemetry.
//!
//! A node may be configured to read *host-wide* measurements from a local
//! telemetry provider and expose them to callers. The feature is opt-in: with
//! no configuration the node creates no provider, starts no task, and claims
//! nothing.
//!
//! Three properties are load-bearing here and are enforced by construction
//! rather than by convention:
//!
//! * **The endpoint is a validated loopback URL.** The configured endpoint is
//!   parsed and checked once, at load time, against a closed rule set: plain
//!   HTTP, an explicit TCP port, an IP literal that is a loopback address, and
//!   exactly the provider's status path. Hostnames — including `localhost` —
//!   are refused, so no DNS answer can move the request off the loopback
//!   interface. The client is built without redirect and proxy support, so a
//!   redirect hop or an inherited `HTTP_PROXY` cannot move it either.
//! * **One owner polls.** A single background task performs every provider
//!   request. Readers consult the cache; they never trigger a provider
//!   request, so a caller cannot amplify into a poll storm.
//! * **Failures are bounded and typed.** Every failure becomes one of a closed
//!   set of categories. No response body, URL, or host path is ever carried
//!   into a category, a log line, or a metric.
//!
//! The measurements themselves are normalized into
//! [`eggwork_core::HostTelemetry`], which carries no provider vocabulary. The
//! provider's own wire types stop at this module's boundary.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use eggfetch_core::{Client, Error as FetchError, Timeout};
use eggwork_core::{
    HostTelemetry, TelemetryCpu, TelemetryDisk, TelemetryDiskIo, TelemetryErrorCategory,
    TelemetryLoad, TelemetryMemory, TelemetryMetric, TelemetryNetwork, TelemetryProviderState,
    TelemetrySwap,
};
use gregg_protocol::{
    ReadinessState,
    v2::{DiskIoPayload, HealthResponseV2, NetworkPayload, SCHEMA_VERSION_V2, StatusPayloadV2},
};
use thiserror::Error;
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

/// The only path the provider protocol serves a status payload on.
const PROVIDER_STATUS_PATH: &str = "/v2/status";

/// Largest decoded provider response the adapter accepts.
///
/// The provider's own payload is a bounded document; a larger body is a
/// defect or an attack, and either way it is refused rather than truncated.
const MAX_PROVIDER_BODY_BYTES: usize = 256 * 1024;

/// Deadline for establishing the loopback connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Deadline for the whole request, including reading the body.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Largest accepted difference between the provider's wall-clock timestamp and
/// the node's own clock.
///
/// A provider timestamp in the future is a clock-skew defect, not a fresh
/// sample, and is refused rather than treated as indefinitely fresh.
const MAX_CLOCK_SKEW: Duration = Duration::from_secs(30);

/// Longest delay between two provider attempts after repeated failures.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Smallest accepted refresh interval.
const MIN_REFRESH_SECONDS: u64 = 1;

/// Largest accepted refresh interval.
const MAX_REFRESH_SECONDS: u64 = 3_600;

/// Largest accepted freshness bound.
const MAX_FRESHNESS_SECONDS: u64 = 3_600;

/// Why a host-telemetry endpoint or cadence was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TelemetryConfigError {
    /// The endpoint is not a plain-HTTP loopback URL with an explicit port and
    /// exactly the provider status path.
    #[error(
        "host telemetry endpoint must be a loopback http URL with an explicit port and the provider status path"
    )]
    InvalidEndpoint,
    /// The refresh interval is outside the accepted range.
    #[error("host telemetry refresh interval is out of range")]
    InvalidRefreshInterval,
    /// The freshness bound is outside the accepted range, or is shorter than
    /// the refresh interval.
    #[error("host telemetry freshness bound is out of range")]
    InvalidFreshness,
}

/// A validated host-telemetry configuration.
///
/// The endpoint is validated once, here, so that no later stage has to
/// re-derive whether it is safe to contact.
#[derive(Debug, Clone)]
pub struct HostTelemetryConfig {
    endpoint: String,
    refresh_interval: Duration,
    freshness: Duration,
}

impl HostTelemetryConfig {
    /// Validate and build a configuration.
    ///
    /// `endpoint` must be a plain-HTTP loopback URL naming an explicit TCP
    /// port and exactly the provider's status path. `refresh_seconds` is how
    /// often the owner polls, and `freshness_seconds` is how long a cached
    /// observation stays fresh; the freshness bound must not be shorter than
    /// the refresh interval.
    pub fn new(
        endpoint: &str,
        refresh_seconds: u64,
        freshness_seconds: u64,
    ) -> Result<Self, TelemetryConfigError> {
        if !(MIN_REFRESH_SECONDS..=MAX_REFRESH_SECONDS).contains(&refresh_seconds) {
            return Err(TelemetryConfigError::InvalidRefreshInterval);
        }
        if !(MIN_REFRESH_SECONDS..=MAX_FRESHNESS_SECONDS).contains(&freshness_seconds) {
            return Err(TelemetryConfigError::InvalidFreshness);
        }
        if freshness_seconds < refresh_seconds {
            return Err(TelemetryConfigError::InvalidFreshness);
        }
        let endpoint = validate_endpoint(endpoint)?;
        Ok(Self {
            endpoint,
            refresh_interval: Duration::from_secs(refresh_seconds),
            freshness: Duration::from_secs(freshness_seconds),
        })
    }

    /// The validated loopback endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// How often the owner polls the provider.
    pub fn refresh_interval(&self) -> Duration {
        self.refresh_interval
    }

    /// How long a cached observation stays fresh.
    pub fn freshness(&self) -> Duration {
        self.freshness
    }
}

/// Validate a provider endpoint and return its normalized form.
///
/// The rule set is closed on purpose. A provider that lives on this machine
/// is addressed by an IP literal, so there is no name to resolve and no
/// answer to be rebound between validation and use. Everything the caller
/// could otherwise smuggle past the check — a scheme other than plain HTTP, a
/// userinfo section, a query string, a fragment, another path, a missing or
/// non-numeric port, a hostname of any kind — is refused.
fn validate_endpoint(raw: &str) -> Result<String, TelemetryConfigError> {
    let parsed = Url::parse(raw).map_err(|_| TelemetryConfigError::InvalidEndpoint)?;
    if parsed.scheme() != "http" {
        return Err(TelemetryConfigError::InvalidEndpoint);
    }
    if parsed.cannot_be_a_base() {
        return Err(TelemetryConfigError::InvalidEndpoint);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(TelemetryConfigError::InvalidEndpoint);
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(TelemetryConfigError::InvalidEndpoint);
    }
    if parsed.path() != PROVIDER_STATUS_PATH {
        return Err(TelemetryConfigError::InvalidEndpoint);
    }
    // The port must be named explicitly. `Url::port` reports `None` both for
    // an endpoint without a port and for one naming the scheme default,
    // because the parser normalizes the latter away, so the authority is
    // inspected directly.
    if !names_explicit_port(raw) {
        return Err(TelemetryConfigError::InvalidEndpoint);
    }
    let Some(port) = parsed.port_or_known_default() else {
        return Err(TelemetryConfigError::InvalidEndpoint);
    };
    if port == 0 {
        return Err(TelemetryConfigError::InvalidEndpoint);
    }
    match parsed.host() {
        Some(Host::Ipv4(address)) if address.is_loopback() => {
            // The endpoint the client is finally given must name the address
            // literally. The URL parser already rewrites the alternative
            // spellings of an IPv4 address to the canonical form, and this
            // check refuses anything it did not.
            if parsed.host_str() != Some(address.to_string().as_str()) {
                return Err(TelemetryConfigError::InvalidEndpoint);
            }
        }
        Some(Host::Ipv6(address)) if address.is_loopback() => {
            if parsed.host_str() != Some(format!("[{address}]").as_str()) {
                return Err(TelemetryConfigError::InvalidEndpoint);
            }
        }
        _ => return Err(TelemetryConfigError::InvalidEndpoint),
    }
    Ok(parsed.to_string())
}

/// Whether the raw endpoint names an explicit TCP port.
///
/// The authority is the text between the scheme and the path. Userinfo is
/// stripped first, and an IPv6 literal is bracketed, so its colons are not a
/// port separator.
fn names_explicit_port(raw: &str) -> bool {
    let Some((_, after_scheme)) = raw.split_once("://") else {
        return false;
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest
            .split_once(']')
            .is_some_and(|(_, tail)| tail.starts_with(':') && tail.len() > 1);
    }
    authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Why a provider attempt did not produce an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TelemetryError {
    /// The loopback connection could not be established.
    #[error("host telemetry provider is unreachable")]
    Connection,
    /// The request exceeded its deadline.
    #[error("host telemetry provider timed out")]
    Timeout,
    /// The provider answered with an unexpected status.
    #[error("host telemetry provider returned an unexpected status")]
    HttpStatus,
    /// The response body was not the expected document.
    #[error("host telemetry provider returned a malformed response")]
    Malformed,
    /// The response carried an unsupported schema version.
    #[error("host telemetry provider returned an unsupported schema version")]
    SchemaVersion,
    /// The response parsed but violated the protocol's invariants.
    #[error("host telemetry provider returned an invalid response")]
    Invalid,
    /// The response exceeded the accepted body size.
    #[error("host telemetry provider response was too large")]
    TooLarge,
    /// The provider's timestamp is outside the accepted clock-skew window.
    #[error("host telemetry provider timestamp is outside the accepted clock-skew window")]
    ClockSkew,
}

impl TelemetryError {
    /// The bounded category reported for this failure.
    pub fn category(&self) -> TelemetryErrorCategory {
        match self {
            Self::Connection => TelemetryErrorCategory::Connection,
            Self::Timeout => TelemetryErrorCategory::Timeout,
            Self::HttpStatus => TelemetryErrorCategory::HttpStatus,
            Self::Malformed => TelemetryErrorCategory::Malformed,
            Self::SchemaVersion => TelemetryErrorCategory::SchemaVersion,
            Self::Invalid => TelemetryErrorCategory::Invalid,
            Self::TooLarge => TelemetryErrorCategory::TooLarge,
            Self::ClockSkew => TelemetryErrorCategory::ClockSkew,
        }
    }
}

/// What a provider attempt produced.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderOutcome {
    /// A validated, normalized observation.
    Observed(HostTelemetry),
    /// The provider is alive but has not produced an observation yet.
    Warming,
}

/// A read-only source of host-wide measurements.
///
/// The seam exists so the node's polling owner can be exercised against a
/// fake, and so a second provider could be added without touching the cache
/// or the node lifecycle.
#[async_trait]
pub trait HostTelemetryProvider: Send + Sync + 'static {
    /// Fetch one observation, or say why there is none.
    async fn observe(&self) -> Result<ProviderOutcome, TelemetryError>;
}

/// The provider adapter that reads a local Gregg daemon.
///
/// The request is issued over plain HTTP to the validated loopback endpoint.
/// The client is built without redirect and proxy support, so a `3xx` is
/// reported as an unexpected status rather than followed, and no ambient
/// `HTTP_PROXY` setting is consulted.
pub struct GreggTelemetryProvider {
    client: Client,
    endpoint: String,
}

impl GreggTelemetryProvider {
    /// Build a provider for a validated configuration.
    pub fn new(config: &HostTelemetryConfig) -> Self {
        Self::with_deadlines(config, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
    }

    /// Build a provider with explicit connection and request deadlines.
    ///
    /// The deadlines bound how long a single provider attempt may take. The
    /// default pair is used by the node; a shorter pair makes a stuck
    /// provider fail faster, and a longer one is only useful for a provider
    /// that is known to answer slowly.
    pub fn with_deadlines(
        config: &HostTelemetryConfig,
        connect: Duration,
        total: Duration,
    ) -> Self {
        let client = Client::builder()
            .timeout(
                Timeout::builder()
                    .connect(connect)
                    .read(total)
                    .total(total)
                    .build(),
            )
            .max_decoded_body_size(MAX_PROVIDER_BODY_BYTES)
            .build();
        Self {
            client,
            endpoint: config.endpoint().to_owned(),
        }
    }
}

#[async_trait]
impl HostTelemetryProvider for GreggTelemetryProvider {
    async fn observe(&self) -> Result<ProviderOutcome, TelemetryError> {
        let mut response = self
            .client
            .get(&self.endpoint)
            .map_err(classify_fetch_error)?
            .send()
            .await
            .map_err(classify_fetch_error)?;
        let status = response.status().as_u16();
        if status == 200 {
            let body = response.bytes().await.map_err(classify_fetch_error)?;
            if body.len() > MAX_PROVIDER_BODY_BYTES {
                return Err(TelemetryError::TooLarge);
            }
            return decode_status(&body);
        }
        if status == 503 {
            // A warming daemon answers with a health envelope rather than a
            // status payload. That is a distinct state, not a failure.
            let body = response.bytes().await.map_err(classify_fetch_error)?;
            if let Ok(health) = serde_json::from_slice::<HealthResponseV2>(&body)
                && health.state == ReadinessState::Warming
            {
                return Ok(ProviderOutcome::Warming);
            }
        }
        Err(TelemetryError::HttpStatus)
    }
}

/// Map a transport failure onto a bounded category.
fn classify_fetch_error(error: FetchError) -> TelemetryError {
    match error {
        FetchError::Timeout { .. } | FetchError::TransportIoTimeout { .. } => {
            TelemetryError::Timeout
        }
        FetchError::DecodedBodyTooLarge | FetchError::DecompressionRatioExceeded => {
            TelemetryError::TooLarge
        }
        FetchError::Body(_) | FetchError::Protocol(_) => TelemetryError::Malformed,
        FetchError::Connect(_)
        | FetchError::CustomTransport(_)
        | FetchError::Io(_)
        | FetchError::Hyper(_)
        | FetchError::HyperClient(_) => TelemetryError::Connection,
        _ => TelemetryError::Connection,
    }
}

/// Decode and validate a provider status payload, then normalize it.
///
/// The provider's own validation is applied first, so a payload that the
/// provider's protocol would reject never reaches the normalized form. The
/// normalized form is validated again, because the two check different
/// things: the provider validates its own schema, and this validates the
/// translation.
fn decode_status(body: &[u8]) -> Result<ProviderOutcome, TelemetryError> {
    let payload: StatusPayloadV2 =
        serde_json::from_slice(body).map_err(|_| TelemetryError::Malformed)?;
    if payload.snapshot.schema_version != SCHEMA_VERSION_V2 {
        return Err(TelemetryError::SchemaVersion);
    }
    payload.validate().map_err(|_| TelemetryError::Invalid)?;
    if exceeds_clock_skew(payload.snapshot.observed_at_unix_ms) {
        return Err(TelemetryError::ClockSkew);
    }
    let observation = normalize(&payload)?;
    observation
        .validate()
        .map_err(|_| TelemetryError::Invalid)?;
    Ok(ProviderOutcome::Observed(observation))
}

/// Translate a validated provider payload into the normalized vocabulary.
///
/// Only host-wide measurements are carried across. The provider's system
/// identity, drive names, device identifiers, and interface details are
/// dropped here and never leave the node.
fn normalize(payload: &StatusPayloadV2) -> Result<HostTelemetry, TelemetryError> {
    let snapshot = &payload.snapshot;
    let capabilities = &snapshot.capabilities;

    let cpu = TelemetryMetric::Value(TelemetryCpu {
        logical_cores: snapshot.cpu.logical_cores,
        usage_pct: snapshot.cpu.usage_pct,
        iowait_pct: capabilities
            .cpu_iowait
            .then_some(snapshot.cpu.iowait_pct)
            .flatten(),
    });

    let memory = TelemetryMetric::Value(TelemetryMemory {
        available_bytes: snapshot
            .memory
            .total_bytes
            .saturating_sub(snapshot.memory.used_bytes),
        total_bytes: snapshot.memory.total_bytes,
        used_bytes: snapshot.memory.used_bytes,
    });

    let load = match (capabilities.load_average, snapshot.load) {
        (true, Some(load)) => TelemetryMetric::Value(TelemetryLoad {
            one: load.one,
            five: load.five,
            fifteen: load.fifteen,
        }),
        (true, None) => TelemetryMetric::Unknown,
        (false, _) => TelemetryMetric::Unsupported,
    };

    let swap = match (capabilities.swap, snapshot.swap) {
        (true, Some(swap)) => TelemetryMetric::Value(TelemetrySwap {
            used_bytes: swap.used_bytes,
            total_bytes: swap.total_bytes,
        }),
        (true, None) => TelemetryMetric::Unknown,
        (false, _) => TelemetryMetric::Unsupported,
    };

    let disk = match payload.drives.as_ref() {
        Some(drives) if !drives.is_empty() => {
            let mut available_bytes = 0u64;
            let mut total_bytes = 0u64;
            for drive in drives {
                available_bytes =
                    available_bytes.saturating_add(drive.available_bytes.unwrap_or(0));
                total_bytes = total_bytes.saturating_add(drive.total_bytes);
            }
            TelemetryMetric::Value(TelemetryDisk {
                available_bytes,
                total_bytes,
                volume_count: u32::try_from(drives.len()).unwrap_or(u32::MAX),
            })
        }
        Some(_) => TelemetryMetric::Unknown,
        None => TelemetryMetric::Unsupported,
    };

    let disk_io = match payload.disk_io.as_ref() {
        Some(disk_io) => TelemetryMetric::Value(normalize_disk_io(disk_io)),
        None => TelemetryMetric::Unsupported,
    };

    let network = match payload.network.as_ref() {
        Some(network) => TelemetryMetric::Value(normalize_network(network)),
        None => TelemetryMetric::Unsupported,
    };

    Ok(HostTelemetry {
        observed_at_unix_ms: snapshot.observed_at_unix_ms,
        sample_interval_ms: snapshot.sample_interval_ms,
        cpu,
        memory,
        load,
        swap,
        disk,
        disk_io,
        network,
    })
}

/// Carry the provider's de-duplicated aggregate disk throughput.
///
/// The per-device records are dropped: they name devices, and the aggregate
/// is the only figure that is meaningful for a host-wide view.
fn normalize_disk_io(disk_io: &DiskIoPayload) -> TelemetryDiskIo {
    TelemetryDiskIo {
        read_bytes_per_sec: disk_io.aggregate_read_bytes_per_sec,
        write_bytes_per_sec: disk_io.aggregate_write_bytes_per_sec,
    }
}

/// Carry the provider's de-duplicated aggregate network throughput.
///
/// The per-interface records are dropped: they name interfaces, and the
/// aggregate is the only figure that is meaningful for a host-wide view.
fn normalize_network(network: &NetworkPayload) -> TelemetryNetwork {
    TelemetryNetwork {
        rx_bytes_per_sec: network.aggregate_rx_bytes_per_sec,
        tx_bytes_per_sec: network.aggregate_tx_bytes_per_sec,
    }
}

/// The cached provider state, as a reader sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetrySnapshot {
    /// The provider's lifecycle state.
    pub state: TelemetryProviderState,
    /// The most recent validated observation, when there is one.
    pub observation: Option<HostTelemetry>,
    /// How long ago, in milliseconds, the node received that observation.
    pub age_ms: Option<u64>,
    /// The category of the most recent failure, when there is one.
    pub last_error: Option<TelemetryErrorCategory>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    state: TelemetryProviderState,
    observation: Option<HostTelemetry>,
    received_at: Option<Instant>,
    provider_stale: bool,
    last_error: Option<TelemetryErrorCategory>,
}

impl CacheEntry {
    fn disabled() -> Self {
        Self {
            state: TelemetryProviderState::Disabled,
            observation: None,
            received_at: None,
            provider_stale: false,
            last_error: None,
        }
    }
}

/// The node's cached view of the host telemetry provider.
///
/// The cache is the only thing a reader consults. It is cheap to clone and
/// cheap to read: a read never waits on the network, and never triggers a
/// provider request.
#[derive(Clone)]
pub struct TelemetryCache {
    entry: Arc<RwLock<CacheEntry>>,
    freshness: Duration,
}

impl TelemetryCache {
    fn new(freshness: Duration) -> Self {
        Self {
            entry: Arc::new(RwLock::new(CacheEntry::disabled())),
            freshness,
        }
    }

    /// A cache for a node with no provider configured.
    pub fn disabled() -> Self {
        Self::new(Duration::from_secs(0))
    }

    /// Read the current provider state.
    ///
    /// The reported age is measured on the node's monotonic clock, so a
    /// wall-clock adjustment cannot make a stale sample look fresh or a fresh
    /// sample look expired. The provider's own wall-clock timestamp stays in
    /// the observation as provenance.
    pub async fn snapshot(&self) -> TelemetrySnapshot {
        let entry = self.entry.read().await;
        let age_ms = entry
            .received_at
            .map(|received_at| received_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        let state = match entry.state {
            TelemetryProviderState::Ready => {
                let aged_out = age_ms.is_some_and(|age| age > self.freshness.as_millis() as u64);
                if entry.provider_stale || aged_out {
                    TelemetryProviderState::Stale
                } else {
                    TelemetryProviderState::Ready
                }
            }
            other => other,
        };
        TelemetrySnapshot {
            state,
            observation: entry.observation.clone(),
            age_ms,
            last_error: entry.last_error,
        }
    }

    /// Record a validated observation.
    async fn record_observation(&self, observation: HostTelemetry, provider_stale: bool) {
        let mut entry = self.entry.write().await;
        entry.state = TelemetryProviderState::Ready;
        entry.observation = Some(observation);
        entry.received_at = Some(Instant::now());
        entry.provider_stale = provider_stale;
        entry.last_error = None;
    }

    /// Record that the provider is alive but has no observation yet.
    async fn record_warming(&self) {
        let mut entry = self.entry.write().await;
        entry.state = TelemetryProviderState::Warming;
        entry.last_error = None;
    }

    /// Record a failure, keeping the last observation for context.
    async fn record_failure(&self, category: TelemetryErrorCategory) {
        let mut entry = self.entry.write().await;
        entry.state = TelemetryProviderState::Unavailable;
        entry.last_error = Some(category);
    }

    /// Record that the provider answered with something unusable.
    async fn record_invalid(&self) {
        let mut entry = self.entry.write().await;
        entry.state = TelemetryProviderState::Invalid;
        entry.last_error = Some(TelemetryErrorCategory::Invalid);
    }
}

/// The node's polling owner for host telemetry.
///
/// Exactly one task performs provider requests. It is the sole refresher, so
/// a burst of readers cannot turn into a burst of provider requests.
#[derive(Clone)]
pub struct TelemetryMonitor {
    cache: TelemetryCache,
    cancellation: CancellationToken,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl TelemetryMonitor {
    /// A monitor for a node with no provider configured.
    ///
    /// No task is created. There is nothing to poll and nothing to claim.
    pub fn disabled() -> Self {
        Self {
            cache: TelemetryCache::disabled(),
            cancellation: CancellationToken::new(),
            task: Arc::new(Mutex::new(None)),
        }
    }

    /// Start the polling owner for a configured provider.
    pub fn start(provider: Arc<dyn HostTelemetryProvider>, config: &HostTelemetryConfig) -> Self {
        let cache = TelemetryCache::new(config.freshness());
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(refresh_loop(
            provider,
            cache.clone(),
            config.refresh_interval(),
            config.freshness(),
            cancellation.clone(),
        ));
        Self {
            cache,
            cancellation,
            task: Arc::new(Mutex::new(Some(task))),
        }
    }

    /// The cache a reader consults.
    pub fn cache(&self) -> &TelemetryCache {
        &self.cache
    }

    /// Stop the polling owner and wait for it to finish.
    pub async fn shutdown(&self) {
        self.cancellation.cancel();
        let task = self.task.lock().await.take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }
}

/// The polling owner.
///
/// The first attempt happens as soon as the node is running: waiting for a
/// whole interval before the first sample would make a freshly started node
/// look empty for no reason. After that the cadence is the configured
/// interval, and failures back off so an absent provider is not polled in a
/// tight loop.
async fn refresh_loop(
    provider: Arc<dyn HostTelemetryProvider>,
    cache: TelemetryCache,
    refresh_interval: Duration,
    freshness: Duration,
    cancellation: CancellationToken,
) {
    let mut failures: u32 = 0;
    loop {
        match provider.observe().await {
            Ok(ProviderOutcome::Observed(observation)) => {
                let provider_stale = is_provider_sample_stale(&observation, freshness);
                cache.record_observation(observation, provider_stale).await;
                failures = 0;
            }
            Ok(ProviderOutcome::Warming) => {
                cache.record_warming().await;
                failures = 0;
            }
            Err(error) => {
                if error == TelemetryError::Invalid {
                    cache.record_invalid().await;
                } else {
                    cache.record_failure(error.category()).await;
                }
                failures = failures.saturating_add(1);
            }
        }
        let delay = if failures == 0 {
            refresh_interval
        } else {
            backoff(refresh_interval, failures)
        };
        tokio::select! {
            _ = cancellation.cancelled() => break,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

/// Whether the provider's own timestamp is already older than the freshness
/// bound, and whether it lies implausibly in the future.
fn is_provider_sample_stale(observation: &HostTelemetry, freshness: Duration) -> bool {
    let Some(now) = wall_clock_millis() else {
        return false;
    };
    if observation.observed_at_unix_ms > now.saturating_add(MAX_CLOCK_SKEW.as_millis() as u64) {
        // A future timestamp is a clock-skew defect. The sample is refused by
        // the caller before it reaches this point, so this is a belt-and-braces
        // check rather than the primary defence.
        return true;
    }
    let age = now.saturating_sub(observation.observed_at_unix_ms);
    age > freshness.as_millis() as u64
}

/// The delay before the next attempt after `failures` consecutive failures.
fn backoff(refresh_interval: Duration, failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(5);
    let scaled = refresh_interval.saturating_mul(1u32 << exponent);
    scaled.min(MAX_BACKOFF)
}

/// The node's wall clock, in Unix milliseconds.
fn wall_clock_millis() -> Option<u64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(duration.as_millis()).ok()
}

/// The clock-skew check applied to a provider timestamp.
pub fn exceeds_clock_skew(observed_at_unix_ms: u64) -> bool {
    let Some(now) = wall_clock_millis() else {
        return false;
    };
    observed_at_unix_ms > now.saturating_add(MAX_CLOCK_SKEW.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg_protocol::{
        LoadAverage, MemoryMetrics, SystemIdentity,
        v2::{CpuMetricsV2, DriveMetrics, MetricCapabilitiesV2, StatusSnapshotV2, SwapMetrics},
    };

    fn identity() -> SystemIdentity {
        SystemIdentity {
            name: "host".into(),
            hostname: "host.local".into(),
            os_name: "linux".into(),
            os_version: "6.1".into(),
            kernel_name: "Linux".into(),
            kernel_release: "6.1.0".into(),
            architecture: "x86_64".into(),
        }
    }

    fn snapshot(capabilities: MetricCapabilitiesV2) -> StatusSnapshotV2 {
        StatusSnapshotV2 {
            schema_version: SCHEMA_VERSION_V2,
            observed_at_unix_ms: 1_700_000_000_000,
            sample_interval_ms: 1_000,
            capabilities,
            system: identity(),
            cpu: CpuMetricsV2 {
                logical_cores: 8,
                usage_pct: 12.5,
                iowait_pct: capabilities.cpu_iowait.then_some(1.5),
            },
            load: capabilities.load_average.then_some(LoadAverage {
                one: 0.5,
                five: 0.25,
                fifteen: 0.125,
            }),
            memory: MemoryMetrics {
                used_bytes: 4_000,
                total_bytes: 8_000,
                usage_pct: 50.0,
            },
            swap: capabilities.swap.then_some(SwapMetrics {
                used_bytes: 0,
                total_bytes: 2_000,
                usage_pct: 0.0,
            }),
            commit: capabilities
                .memory_commit
                .then_some(gregg_protocol::v2::CommitMetrics {
                    used_bytes: 1,
                    limit_bytes: 2,
                    usage_pct: 50.0,
                }),
        }
    }

    fn payload(capabilities: MetricCapabilitiesV2) -> StatusPayloadV2 {
        StatusPayloadV2 {
            snapshot: snapshot(capabilities),
            drives: None,
            cpu_frequency_hz: None,
            disk_io: None,
            network: None,
        }
    }

    #[test]
    fn a_loopback_endpoint_is_accepted() {
        let config = HostTelemetryConfig::new("http://127.0.0.1:18080/v2/status", 15, 60).unwrap();
        assert_eq!(config.endpoint(), "http://127.0.0.1:18080/v2/status");
        let config = HostTelemetryConfig::new("http://[::1]:18080/v2/status", 15, 60).unwrap();
        assert_eq!(config.endpoint(), "http://[::1]:18080/v2/status");
    }

    #[test]
    fn a_non_loopback_or_ambiguous_endpoint_is_refused() {
        for endpoint in [
            // A name would have to be resolved, and a resolved answer could be
            // rebound between validation and use.
            "http://localhost:18080/v2/status",
            "http://example.com:18080/v2/status",
            // A non-loopback address is not this machine.
            "http://10.0.0.1:18080/v2/status",
            "http://192.168.1.10:18080/v2/status",
            "http://[fe80::1]:18080/v2/status",
            // An IPv4-mapped loopback address is not the loopback address.
            "http://[::ffff:127.0.0.1]:18080/v2/status",
            // The provider speaks plain HTTP on the loopback interface.
            "https://127.0.0.1:18080/v2/status",
            "ftp://127.0.0.1:18080/v2/status",
            "file:///etc/passwd",
            // Anything that changes the request's target or shape.
            "http://user:pass@127.0.0.1:18080/v2/status",
            "http://user@127.0.0.1:18080/v2/status",
            "http://127.0.0.1:18080/v2/status?probe=1",
            "http://127.0.0.1:18080/v2/status#fragment",
            "http://127.0.0.1:18080/",
            "http://127.0.0.1:18080/v2/other",
            "http://127.0.0.1:18080/v2/status/extra",
            "http://127.0.0.1/v2/status",
            "http://127.0.0.1:0/v2/status",
            "http://127.0.0.1:99999/v2/status",
        ] {
            assert!(
                HostTelemetryConfig::new(endpoint, 15, 60).is_err(),
                "endpoint {endpoint} must be refused"
            );
        }
    }

    #[test]
    fn an_alternative_numeric_spelling_is_canonicalized_to_the_address() {
        // These all name the loopback address, and the URL parser rewrites
        // them to the literal address. The endpoint the client is given is
        // therefore the literal address, and no name is ever resolved.
        for endpoint in [
            "http://127.0.0.1.:18080/v2/status",
            "http://2130706433:18080/v2/status",
            "http://0x7f000001:18080/v2/status",
            "http://127.1:18080/v2/status",
            "http://127.000.000.001:18080/v2/status",
        ] {
            let config = HostTelemetryConfig::new(endpoint, 15, 60).unwrap();
            assert_eq!(config.endpoint(), "http://127.0.0.1:18080/v2/status");
        }
        let config =
            HostTelemetryConfig::new("http://[0:0:0:0:0:0:0:1]:18080/v2/status", 15, 60).unwrap();
        assert_eq!(config.endpoint(), "http://[::1]:18080/v2/status");
    }

    #[test]
    fn an_out_of_range_cadence_is_refused() {
        assert_eq!(
            HostTelemetryConfig::new("http://127.0.0.1:18080/v2/status", 0, 60).unwrap_err(),
            TelemetryConfigError::InvalidRefreshInterval
        );
        assert_eq!(
            HostTelemetryConfig::new("http://127.0.0.1:18080/v2/status", 3_601, 60).unwrap_err(),
            TelemetryConfigError::InvalidRefreshInterval
        );
        assert_eq!(
            HostTelemetryConfig::new("http://127.0.0.1:18080/v2/status", 15, 0).unwrap_err(),
            TelemetryConfigError::InvalidFreshness
        );
        assert_eq!(
            HostTelemetryConfig::new("http://127.0.0.1:18080/v2/status", 15, 3_601).unwrap_err(),
            TelemetryConfigError::InvalidFreshness
        );
        // A freshness bound shorter than the poll interval would make every
        // sample stale before the next poll replaced it.
        assert_eq!(
            HostTelemetryConfig::new("http://127.0.0.1:18080/v2/status", 30, 15).unwrap_err(),
            TelemetryConfigError::InvalidFreshness
        );
    }

    #[test]
    fn a_supported_measurement_is_carried_and_an_unsupported_one_is_not() {
        let payload = payload(MetricCapabilitiesV2::new(true, true, true, false));
        let observation = normalize(&payload).unwrap();
        assert_eq!(observation.cpu.value().unwrap().logical_cores, 8);
        assert_eq!(observation.cpu.value().unwrap().iowait_pct, Some(1.5));
        assert!(observation.load.value().is_some());
        assert!(observation.swap.value().is_some());
        // No drive, disk-I/O or network data was reported.
        assert!(observation.disk.is_unsupported());
        assert!(observation.disk_io.is_unsupported());
        assert!(observation.network.is_unsupported());
    }

    #[test]
    fn an_unsupported_capability_is_not_reported_as_a_missing_value() {
        let payload = payload(MetricCapabilitiesV2::new(false, false, false, false));
        let observation = normalize(&payload).unwrap();
        assert_eq!(observation.cpu.value().unwrap().iowait_pct, None);
        assert!(observation.load.is_unsupported());
        assert!(observation.swap.is_unsupported());
        // A missing measurement is never a zero.
        assert!(observation.load.value().is_none());
    }

    #[test]
    fn a_supported_but_unreported_measurement_is_unknown() {
        let mut payload = payload(MetricCapabilitiesV2::new(true, true, true, false));
        payload.snapshot.load = None;
        let observation = normalize(&payload).unwrap();
        assert!(observation.load.is_unknown());
        assert!(!observation.load.is_unsupported());
    }

    #[test]
    fn drive_and_throughput_data_is_aggregated_without_identifiers() {
        let mut payload = payload(MetricCapabilitiesV2::new(false, false, false, false));
        payload.drives = Some(vec![
            DriveMetrics {
                name: "/".into(),
                used_bytes: 100,
                total_bytes: 1_000,
                available_bytes: Some(900),
            },
            DriveMetrics {
                name: "/data".into(),
                used_bytes: 50,
                total_bytes: 500,
                available_bytes: Some(450),
            },
        ]);
        payload.disk_io = Some(gregg_protocol::v2::DiskIoPayload {
            aggregate_read_bytes_per_sec: 11,
            aggregate_write_bytes_per_sec: 22,
            devices: vec![gregg_protocol::v2::DiskIoMetrics {
                id: "sda".into(),
                name: "sda".into(),
                read_bytes_per_sec: 11,
                write_bytes_per_sec: 22,
                drive_name: Some("/".into()),
            }],
        });
        payload.network = Some(gregg_protocol::v2::NetworkPayload {
            aggregate_rx_bytes_per_sec: 33,
            aggregate_tx_bytes_per_sec: 44,
            aggregate_rx_capacity_bps: None,
            aggregate_tx_capacity_bps: None,
            interfaces: vec![],
        });
        let observation = normalize(&payload).unwrap();
        let disk = observation.disk.value().unwrap();
        assert_eq!(disk.available_bytes, 1_350);
        assert_eq!(disk.total_bytes, 1_500);
        assert_eq!(disk.volume_count, 2);
        assert_eq!(observation.disk_io.value().unwrap().read_bytes_per_sec, 11);
        assert_eq!(observation.network.value().unwrap().rx_bytes_per_sec, 33);
        // The normalized form carries no drive name, device identity, or
        // interface identity: only aggregates.
        let encoded = serde_json::to_string(&observation).unwrap();
        assert!(!encoded.contains("/data"));
        assert!(!encoded.contains("sda"));
    }

    #[test]
    fn an_invalid_provider_payload_is_refused() {
        let mut payload = payload(MetricCapabilitiesV2::new(false, false, false, false));
        payload.snapshot.cpu.usage_pct = 1000.0;
        assert!(payload.validate().is_err());
    }

    #[test]
    fn a_provider_timestamp_in_the_future_is_refused() {
        let mut payload = payload(MetricCapabilitiesV2::new(false, false, false, false));
        payload.snapshot.observed_at_unix_ms = wall_clock_millis().unwrap_or_default() + 600_000;
        let body = serde_json::to_vec(&payload).unwrap();
        assert_eq!(decode_status(&body).unwrap_err(), TelemetryError::ClockSkew);
    }

    #[test]
    fn a_malformed_body_is_refused() {
        assert_eq!(
            decode_status(b"not json").unwrap_err(),
            TelemetryError::Malformed
        );
        assert_eq!(decode_status(b"{}").unwrap_err(), TelemetryError::Malformed);
    }

    #[test]
    fn a_wrong_schema_version_is_refused() {
        let mut payload = payload(MetricCapabilitiesV2::new(false, false, false, false));
        payload.snapshot.schema_version = 1;
        let body = serde_json::to_vec(&payload).unwrap();
        assert_eq!(
            decode_status(&body).unwrap_err(),
            TelemetryError::SchemaVersion
        );
    }

    #[test]
    fn the_backoff_grows_and_is_capped() {
        let interval = Duration::from_secs(15);
        assert_eq!(backoff(interval, 1), Duration::from_secs(15));
        assert_eq!(backoff(interval, 2), Duration::from_secs(30));
        assert_eq!(backoff(interval, 3), Duration::from_secs(60));
        assert_eq!(backoff(interval, 10), MAX_BACKOFF);
    }

    #[tokio::test]
    async fn a_disabled_cache_reports_disabled() {
        let cache = TelemetryCache::disabled();
        let snapshot = cache.snapshot().await;
        assert_eq!(snapshot.state, TelemetryProviderState::Disabled);
        assert!(snapshot.observation.is_none());
        assert!(snapshot.age_ms.is_none());
        assert!(snapshot.last_error.is_none());
    }
}
