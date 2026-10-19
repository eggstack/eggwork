//! Behavioural tests for the optional host-telemetry adapter.
//!
//! The provider side is exercised against a real loopback HTTP server rather
//! than a mocked transport, because the properties that matter here — a
//! redirect that is not followed, a body that is refused for being too large,
//! a response that never arrives — are properties of the transport, not of a
//! stub.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use eggwork_core::{
    HostTelemetry, TelemetryCpu, TelemetryErrorCategory, TelemetryMemory, TelemetryMetric,
    TelemetryProviderState,
};
use eggwork_server::telemetry::{
    GreggTelemetryProvider, HostTelemetryConfig, HostTelemetryProvider, ProviderOutcome,
    TelemetryError, TelemetryMonitor, TelemetrySnapshot,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::Instant,
};

/// A canned HTTP response.
struct Canned {
    status: u16,
    body: Vec<u8>,
    delay: Duration,
}

impl Canned {
    fn json(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// A loopback HTTP server that answers with canned responses.
struct FakeProvider {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
}

impl FakeProvider {
    async fn start(responder: impl Fn(usize) -> Canned + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let responder: Arc<dyn Fn(usize) -> Canned + Send + Sync> = Arc::new(responder);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let counter = counter.clone();
                let responder = responder.clone();
                tokio::spawn(async move {
                    let _ = serve_one(stream, counter, responder.as_ref()).await;
                });
            }
        });
        Self { address, requests }
    }

    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/v2/status", self.address.port())
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

async fn serve_one(
    mut stream: TcpStream,
    counter: Arc<AtomicUsize>,
    responder: &(dyn Fn(usize) -> Canned + Send + Sync),
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let index = counter.fetch_add(1, Ordering::SeqCst);
    let canned = responder(index);
    if !canned.delay.is_zero() {
        tokio::time::sleep(canned.delay).await;
    }
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        canned.status,
        reason(canned.status),
        canned.body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&canned.body).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        302 => "Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// A provider status payload in the shape the provider protocol defines.
fn provider_payload() -> String {
    serde_json::json!({
        "schema_version": 2,
        "observed_at_unix_ms": now_unix_ms(),
        "sample_interval_ms": 1000,
        "capabilities": {
            "cpu_iowait": true,
            "load_average": true,
            "swap": false,
            "memory_commit": false
        },
        "system": {
            "name": "host",
            "hostname": "host.internal",
            "os_name": "linux",
            "os_version": "6.1",
            "kernel_name": "Linux",
            "kernel_release": "6.1.0",
            "architecture": "x86_64"
        },
        "cpu": {
            "logical_cores": 8,
            "usage_pct": 12.5,
            "iowait_pct": 1.5
        },
        "load": { "one": 0.5, "five": 0.25, "fifteen": 0.125 },
        "memory": { "used_bytes": 4000, "total_bytes": 8000, "usage_pct": 50.0 },
        "drives": [
            { "name": "/", "used_bytes": 100, "total_bytes": 1000, "available_bytes": 900 }
        ],
        "disk_io": {
            "aggregate_read_bytes_per_sec": 11,
            "aggregate_write_bytes_per_sec": 22,
            "devices": []
        },
        "network": {
            "aggregate_rx_bytes_per_sec": 33,
            "aggregate_tx_bytes_per_sec": 44,
            "interfaces": []
        }
    })
    .to_string()
}

/// A provider health envelope for a daemon that is still warming up.
fn warming_payload() -> String {
    serde_json::json!({
        "schema_version": 2,
        "state": "warming",
        "category": "warming",
        "message": "collector warming up"
    })
    .to_string()
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64
}

fn config(endpoint: &str) -> HostTelemetryConfig {
    HostTelemetryConfig::new(endpoint, 3_600, 3_600).expect("config")
}

/// Wait until the cache reports a state, or fail the test.
async fn await_state(
    monitor: &TelemetryMonitor,
    expected: TelemetryProviderState,
) -> TelemetrySnapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = monitor.cache().snapshot().await;
        if snapshot.state == expected {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "provider state stayed {snapshot:?} instead of reaching {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_valid_provider_payload_becomes_a_normalized_observation() {
    let provider = FakeProvider::start(|_| Canned::json(200, provider_payload())).await;
    let settings = config(&provider.endpoint());
    let monitor =
        TelemetryMonitor::start(Arc::new(GreggTelemetryProvider::new(&settings)), &settings);
    let snapshot = await_state(&monitor, TelemetryProviderState::Ready).await;
    let observation = snapshot.observation.expect("observation");
    assert_eq!(observation.cpu.value().unwrap().logical_cores, 8);
    assert_eq!(observation.cpu.value().unwrap().iowait_pct, Some(1.5));
    assert_eq!(
        observation.memory.value().unwrap().available_bytes,
        4_000,
        "available memory is the complement of used memory"
    );
    assert_eq!(observation.load.value().unwrap().one, 0.5);
    assert!(
        observation.swap.is_unsupported(),
        "a platform without swap reports an unsupported measurement, not a zero"
    );
    let disk = observation.disk.value().unwrap();
    assert_eq!(disk.available_bytes, 900);
    assert_eq!(disk.total_bytes, 1_000);
    assert_eq!(disk.volume_count, 1);
    assert_eq!(observation.disk_io.value().unwrap().read_bytes_per_sec, 11);
    assert_eq!(observation.network.value().unwrap().tx_bytes_per_sec, 44);
    // The provider's identity, drive name, and interface details never leave
    // the node.
    let encoded = serde_json::to_string(&observation).unwrap();
    assert!(!encoded.contains("host.internal"));
    monitor.shutdown().await;
}

#[tokio::test]
async fn a_warming_provider_is_reported_as_warming() {
    let provider = FakeProvider::start(|_| Canned::json(503, warming_payload())).await;
    let settings = config(&provider.endpoint());
    let monitor =
        TelemetryMonitor::start(Arc::new(GreggTelemetryProvider::new(&settings)), &settings);
    let snapshot = await_state(&monitor, TelemetryProviderState::Warming).await;
    assert!(snapshot.observation.is_none());
    assert!(snapshot.last_error.is_none());
    monitor.shutdown().await;
}

#[tokio::test]
async fn an_unexpected_status_is_reported_as_unavailable() {
    let provider = FakeProvider::start(|_| Canned::json(500, "{}".to_string())).await;
    let settings = config(&provider.endpoint());
    let monitor =
        TelemetryMonitor::start(Arc::new(GreggTelemetryProvider::new(&settings)), &settings);
    let snapshot = await_state(&monitor, TelemetryProviderState::Unavailable).await;
    assert_eq!(
        snapshot.last_error,
        Some(TelemetryErrorCategory::HttpStatus)
    );
    monitor.shutdown().await;
}

#[tokio::test]
async fn a_malformed_body_is_reported_as_unavailable() {
    let provider = FakeProvider::start(|_| Canned::json(200, "not json".to_string())).await;
    let settings = config(&provider.endpoint());
    let monitor =
        TelemetryMonitor::start(Arc::new(GreggTelemetryProvider::new(&settings)), &settings);
    let snapshot = await_state(&monitor, TelemetryProviderState::Unavailable).await;
    assert_eq!(snapshot.last_error, Some(TelemetryErrorCategory::Malformed));
    monitor.shutdown().await;
}

#[tokio::test]
async fn an_oversized_body_is_refused() {
    let oversized = vec![b'x'; 512 * 1024];
    let provider = FakeProvider::start(move |_| Canned::json(200, oversized.clone())).await;
    let settings = config(&provider.endpoint());
    let monitor =
        TelemetryMonitor::start(Arc::new(GreggTelemetryProvider::new(&settings)), &settings);
    let snapshot = await_state(&monitor, TelemetryProviderState::Unavailable).await;
    assert_eq!(snapshot.last_error, Some(TelemetryErrorCategory::TooLarge));
    monitor.shutdown().await;
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let provider = FakeProvider::start(|_| {
        Canned::json(
            302,
            serde_json::json!({ "location": "http://10.0.0.1/v2/status" }).to_string(),
        )
    })
    .await;
    let settings = config(&provider.endpoint());
    let monitor =
        TelemetryMonitor::start(Arc::new(GreggTelemetryProvider::new(&settings)), &settings);
    let snapshot = await_state(&monitor, TelemetryProviderState::Unavailable).await;
    assert_eq!(
        snapshot.last_error,
        Some(TelemetryErrorCategory::HttpStatus)
    );
    assert_eq!(
        provider.requests(),
        1,
        "a redirect must not produce a second request"
    );
    monitor.shutdown().await;
}

#[tokio::test]
async fn a_provider_that_never_answers_times_out() {
    let provider = FakeProvider::start(|_| {
        Canned::json(200, provider_payload()).delayed(Duration::from_secs(30))
    })
    .await;
    let settings = config(&provider.endpoint());
    let monitor = TelemetryMonitor::start(
        Arc::new(GreggTelemetryProvider::with_deadlines(
            &settings,
            Duration::from_millis(200),
            Duration::from_millis(300),
        )),
        &settings,
    );
    let snapshot = await_state(&monitor, TelemetryProviderState::Unavailable).await;
    assert_eq!(snapshot.last_error, Some(TelemetryErrorCategory::Timeout));
    monitor.shutdown().await;
}

#[tokio::test]
async fn an_unreachable_provider_is_reported_as_unavailable() {
    // Bind a port and drop it, so the endpoint is closed rather than
    // filtered. A refused connection is a connection failure, not a timeout.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");
    drop(listener);
    let settings = config(&format!("http://127.0.0.1:{}/v2/status", address.port()));
    let monitor = TelemetryMonitor::start(
        Arc::new(GreggTelemetryProvider::with_deadlines(
            &settings,
            Duration::from_millis(200),
            Duration::from_millis(500),
        )),
        &settings,
    );
    let snapshot = await_state(&monitor, TelemetryProviderState::Unavailable).await;
    assert_eq!(
        snapshot.last_error,
        Some(TelemetryErrorCategory::Connection)
    );
    monitor.shutdown().await;
}

/// A provider that counts how often it was asked for an observation.
struct CountingProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl HostTelemetryProvider for CountingProvider {
    async fn observe(&self) -> Result<ProviderOutcome, TelemetryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderOutcome::Observed(observation()))
    }
}

fn observation() -> HostTelemetry {
    HostTelemetry {
        observed_at_unix_ms: now_unix_ms(),
        sample_interval_ms: 1_000,
        cpu: TelemetryMetric::Value(TelemetryCpu {
            logical_cores: 4,
            usage_pct: 1.0,
            iowait_pct: None,
        }),
        memory: TelemetryMetric::Value(TelemetryMemory {
            available_bytes: 100,
            total_bytes: 200,
            used_bytes: 100,
        }),
        load: TelemetryMetric::Unsupported,
        swap: TelemetryMetric::Unsupported,
        disk: TelemetryMetric::Unsupported,
        disk_io: TelemetryMetric::Unsupported,
        network: TelemetryMetric::Unsupported,
    }
}

#[tokio::test]
async fn one_owner_polls_for_every_reader() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(CountingProvider {
        calls: calls.clone(),
    });
    let settings = config("http://127.0.0.1:1/v2/status");
    let monitor = TelemetryMonitor::start(provider, &settings);
    await_state(&monitor, TelemetryProviderState::Ready).await;
    let after_first = calls.load(Ordering::SeqCst);
    assert_eq!(after_first, 1);

    // A burst of readers must not turn into a burst of provider requests.
    let mut readers = Vec::new();
    for _ in 0..64 {
        let cache = monitor.cache().clone();
        readers.push(tokio::spawn(async move { cache.snapshot().await }));
    }
    for reader in readers {
        let snapshot = reader.await.expect("reader");
        assert_eq!(snapshot.state, TelemetryProviderState::Ready);
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        after_first,
        "readers must be served from the cache, not from the provider"
    );
    monitor.shutdown().await;
}

#[tokio::test]
async fn a_disabled_monitor_never_polls() {
    let monitor = TelemetryMonitor::disabled();
    let snapshot = monitor.cache().snapshot().await;
    assert_eq!(snapshot.state, TelemetryProviderState::Disabled);
    assert!(snapshot.observation.is_none());
    monitor.shutdown().await;
}

#[tokio::test]
async fn a_stopped_monitor_stops_polling() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(CountingProvider {
        calls: calls.clone(),
    });
    let settings = config("http://127.0.0.1:1/v2/status");
    let monitor = TelemetryMonitor::start(provider, &settings);
    await_state(&monitor, TelemetryProviderState::Ready).await;
    monitor.shutdown().await;
    let after_shutdown = calls.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        after_shutdown,
        "a stopped monitor must not keep polling"
    );
}

#[tokio::test]
async fn a_stale_observation_is_reported_as_stale() {
    let provider = FakeProvider::start(|_| {
        // A sample that is already older than the freshness bound.
        let mut payload: serde_json::Value =
            serde_json::from_str(&provider_payload()).expect("payload");
        payload["observed_at_unix_ms"] = serde_json::json!(now_unix_ms() - 3_600_000);
        Canned::json(200, payload.to_string())
    })
    .await;
    let settings = HostTelemetryConfig::new(&provider.endpoint(), 1, 2).expect("config");
    let monitor =
        TelemetryMonitor::start(Arc::new(GreggTelemetryProvider::new(&settings)), &settings);
    let snapshot = await_state(&monitor, TelemetryProviderState::Stale).await;
    assert!(snapshot.observation.is_some());
    monitor.shutdown().await;
}
