//! Installed-release execution qualification (Operations M004).
//!
//! Everything else in the M004 qualification surface exercises the product's
//! own operator commands. This suite exercises the one property those commands
//! cannot reach: that the *installed* release binary, exactly as it was staged
//! and digest-verified by `scripts/qualify_release.py`, accepts a real
//! authenticated loopback execution and admits or rejects capabilities
//! truthfully.
//!
//! It is deliberately `#[ignore]`d. An ordinary `cargo test --workspace` run
//! has no installed release to drive, and a test that silently passes because
//! its subject is missing is worse than no test. Qualification therefore names
//! the subject explicitly through the environment, and an unset subject is an
//! error rather than a skip:
//!
//! ```text
//! EGGWORK_QUALIFY_DAEMON   absolute path to the installed eggworkd
//! EGGWORK_QUALIFY_CONFIG   absolute path to the qualification node config
//! EGGWORK_QUALIFY_TLS_DIR  directory holding ca.pem / client.pem / client-key.pem
//! EGGWORK_QUALIFY_RECEIPT  optional path to write the JSON receipt
//! EGGWORK_QUALIFY_PLATFORM `linux` | `macos` | `windows`
//! ```
//!
//! The node is started as a bounded child of the installed binary and driven
//! through the repository's own `eggwork-client` over mTLS, so the code path
//! under test is the production client path rather than a bespoke probe.

#![cfg(feature = "qualification")]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use eggwork_client::NodeClient;
use eggwork_core::{
    CommandSpec, EnvironmentEntry, ExecutionGeneration, ExecutionHandle, ExecutionId,
    ExecutionSpec, ExecutionState, IsolationRequirement, LeaseId, NetworkRequirement, OutputPolicy,
    Requirement, ResourceRequirements, StdinPolicy,
};
use serde_json::{Value as Json, json};
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_STDOUT_BYTES: u64 = 8 * 1024;
const MAX_READY_OUTPUT_BYTES: usize = 64 * 1024;

/// The qualification subject, or an error explaining what is missing.
struct Subject {
    daemon: PathBuf,
    config: PathBuf,
    tls_dir: PathBuf,
    receipt: Option<PathBuf>,
    platform: String,
}

impl Subject {
    fn from_environment() -> Result<Self, String> {
        let required = |name: &str| {
            std::env::var(name).map_err(|_| {
                format!("{name} is not set; this suite qualifies an installed release")
            })
        };
        let subject = Self {
            daemon: PathBuf::from(required("EGGWORK_QUALIFY_DAEMON")?),
            config: PathBuf::from(required("EGGWORK_QUALIFY_CONFIG")?),
            tls_dir: PathBuf::from(required("EGGWORK_QUALIFY_TLS_DIR")?),
            receipt: std::env::var("EGGWORK_QUALIFY_RECEIPT")
                .ok()
                .map(PathBuf::from),
            platform: std::env::var("EGGWORK_QUALIFY_PLATFORM")
                .unwrap_or_else(|_| std::env::consts::OS.to_owned()),
        };
        for (label, path) in [
            ("EGGWORK_QUALIFY_DAEMON", &subject.daemon),
            ("EGGWORK_QUALIFY_CONFIG", &subject.config),
        ] {
            if !path.is_absolute() {
                return Err(format!(
                    "{label} must be an absolute path, got {}",
                    path.display()
                ));
            }
        }
        if !subject.daemon.is_file() {
            return Err(format!(
                "{} is not an installed file",
                subject.daemon.display()
            ));
        }
        if !subject.config.is_file() {
            return Err(format!("{} is not a file", subject.config.display()));
        }
        Ok(subject)
    }

    fn tls(&self, name: &str) -> PathBuf {
        self.tls_dir.join(name)
    }
}

/// A started installed node, terminated on drop so a failed qualification never
/// leaves an orphan daemon holding the loopback port.
struct InstalledNode {
    child: Child,
    endpoint: String,
    address: std::net::SocketAddr,
}

impl Drop for InstalledNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read the installed daemon's single-line ready report, or fail closed.
fn read_ready_line(stdout: &mut std::process::ChildStdout) -> Result<Json, String> {
    use std::io::Read;
    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    let mut pending = String::new();
    let mut chunk = [0u8; 1024];
    loop {
        if std::time::Instant::now() >= deadline {
            return Err("the installed daemon did not report ready within its bound".into());
        }
        let read = stdout
            .read(&mut chunk)
            .map_err(|error| format!("reading the installed daemon output failed: {error}"))?;
        if read == 0 {
            return Err("the installed daemon exited before reporting ready".into());
        }
        pending.push_str(&String::from_utf8_lossy(&chunk[..read]));
        if pending.len() > MAX_READY_OUTPUT_BYTES {
            return Err(
                "the installed daemon produced unbounded output before reporting ready".into(),
            );
        }
        if let Some(report) = pending
            .lines()
            .last()
            .and_then(|line| serde_json::from_str::<Json>(line.trim()).ok())
            .filter(|report| report.get("ready").and_then(Json::as_bool) == Some(true))
        {
            return Ok(report);
        }
    }
}

/// Pick a free loopback port by binding and immediately releasing it.
fn reserve_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve a loopback port");
    listener.local_addr().expect("reserved address").port()
}

async fn start_installed_node(subject: &Subject, port: u16) -> Result<InstalledNode, String> {
    let config = std::fs::read_to_string(&subject.config).map_err(|error| error.to_string())?;
    let bound = config.replace(
        "\"bind\": \"127.0.0.1:0\"",
        &format!("\"bind\": \"127.0.0.1:{port}\""),
    );
    if bound == config {
        return Err("the qualification config does not declare the expected loopback bind".into());
    }
    let prepared = subject.config.with_extension("qualification.json");
    std::fs::write(&prepared, bound).map_err(|error| error.to_string())?;
    // The installed loader refuses a config any other user can write to, which
    // is the same policy a real operator config must satisfy.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&prepared, std::fs::Permissions::from_mode(0o644))
            .map_err(|error| error.to_string())?;
    }

    // A blocking child is deliberate: `eggwork-server` must never enable Tokio
    // process creation in production, so the qualification harness uses the
    // standard library and confines the blocking read to one task.
    let mut child = Command::new(&subject.daemon)
        .arg("run")
        .arg("--config")
        .arg(&prepared)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawning the installed daemon failed: {error}"))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let ready = tokio::task::spawn_blocking(move || read_ready_line(&mut stdout))
        .await
        .map_err(|error| format!("waiting for the installed daemon output failed: {error}"))??;
    let local = ready
        .get("local_addr")
        .and_then(Json::as_str)
        .ok_or_else(|| "the installed daemon reported ready without a local address".to_owned())?
        .to_owned();
    let address: std::net::SocketAddr = local
        .parse()
        .map_err(|_| format!("unusable local address {local:?}"))?;
    if !address.ip().is_loopback() {
        return Err(format!(
            "the installed daemon bound a non-loopback address {local}"
        ));
    }
    Ok(InstalledNode {
        child,
        endpoint: format!("https://localhost:{port}"),
        address,
    })
}

fn client(subject: &Subject, node: &InstalledNode) -> Result<NodeClient, String> {
    let _ = node.address;
    NodeClient::from_pem_files(
        &node.endpoint,
        subject.tls("ca.pem"),
        subject.tls("client.pem"),
        subject.tls("client-key.pem"),
    )
    .map_err(|error| {
        format!("the production client could not use the qualification identity: {error}")
    })
}

/// A per-run identifier.
///
/// Execution identity is fenced and terminal records survive across runs, so a
/// fixed identifier would collide with the previous run's retained record and
/// report `execution_identity_conflict` instead of qualifying anything.
fn run_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}-{:x}", std::process::id())
}

fn qualified_id(label: &str) -> String {
    format!("qual-{label}-{}", run_id())
}

fn handle(id: &str) -> ExecutionHandle {
    ExecutionHandle {
        execution_id: ExecutionId::new(id).expect("execution id"),
        generation: ExecutionGeneration::new(1).expect("generation"),
        lease_id: LeaseId::new(format!("lease-{id}")).expect("lease"),
    }
}

fn spec(argv: Vec<String>, isolation: IsolationRequirement) -> ExecutionSpec {
    ExecutionSpec {
        schema_version: 1,
        command: CommandSpec {
            argv,
            cwd: None,
            environment: vec![EnvironmentEntry::new("PATH", "/usr/bin:/bin").expect("entry")],
            stdin: StdinPolicy::Null,
            timeout_millis: 30_000,
            output: OutputPolicy::default(),
            declared_outputs: vec![],
            resources: ResourceRequirements {
                memory_bytes: Requirement::NotRequested,
                cpu_millis: Requirement::NotRequested,
                pids: Requirement::NotRequested,
            },
            isolation,
            network: NetworkRequirement::Unrestricted,
        },
        metadata: vec![],
    }
}

fn marker_command(marker: &Path, required: bool) -> Vec<String> {
    if cfg!(windows) {
        vec![
            "cmd.exe".to_owned(),
            "/C".to_owned(),
            format!("echo negative-control > {}", marker.display()),
        ]
    } else {
        let script = if required {
            format!("echo negative-control > {}", marker.display())
        } else {
            "printf 'eggwork-qualification\\n'".to_owned()
        };
        vec!["/bin/sh".to_owned(), "-c".to_owned(), script]
    }
}

async fn wait_for_terminal(
    client: &NodeClient,
    id: &ExecutionId,
) -> Result<eggwork_core::ExecutionSnapshot, String> {
    let deadline = tokio::time::Instant::now() + EXECUTION_TIMEOUT;
    loop {
        let snapshot = client
            .observe(id)
            .await
            .map_err(|error| format!("observing the installed node's execution failed: {error}"))?;
        if matches!(
            snapshot.state,
            ExecutionState::Succeeded
                | ExecutionState::Failed
                | ExecutionState::Cancelled
                | ExecutionState::TimedOut
                | ExecutionState::Interrupted
        ) {
            return Ok(snapshot);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("the installed node's execution never reached a terminal state".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Run one qualified execution through the production client path and report
/// the terminal facts.
async fn qualify_execution(
    client: &NodeClient,
    argv: Vec<String>,
    isolation: IsolationRequirement,
    id: &str,
) -> Result<Json, String> {
    let execution_id =
        ExecutionId::new(id).map_err(|_| format!("{id} is not a usable execution id"))?;
    let stream = tokio::time::timeout(
        EXECUTION_TIMEOUT,
        client.execute(&spec(argv, isolation), &handle(id)),
    )
    .await
    .map_err(|_| "the installed node did not answer the execute request in time".to_owned())?
    .map_err(|error| format!("the installed node refused the execution: {error}"))?;
    use futures_util::StreamExt;
    let mut events = Box::pin(stream.into_events());
    let mut stdout = Vec::new();
    while let Some(event) = events.next().await {
        let event = event.map_err(|error| format!("reading execution events failed: {error}"))?;
        if let eggwork_core::ExecutionEventKind::Stdout(chunk) = event.kind {
            stdout.extend_from_slice(&chunk);
            if stdout.len() as u64 > MAX_STDOUT_BYTES {
                return Err("the installed node's execution exceeded the bounded output".into());
            }
        }
    }
    let snapshot = wait_for_terminal(client, &execution_id).await?;
    let text = String::from_utf8_lossy(&stdout).to_string();
    Ok(json!({
        "execution_id": id,
        "state": format!("{:?}", snapshot.state),
        "exit_code": snapshot.result.as_ref().and_then(|result| result.exit_code),
        "stdout": text,
        "stdout_bytes": snapshot.result.as_ref().map(|result| result.stdout_bytes),
        "cleanup_warning": snapshot.result.as_ref().and_then(|result| result.cleanup_warning.clone()),
        "failure": snapshot.result.as_ref().and_then(|result| result.failure.clone()),
        "sandbox": snapshot.result.as_ref().and_then(|result| result.sandbox.clone()),
    }))
}

/// Run one bounded installed-operator command against the qualification config.
///
/// `operator_command` and `doctor` are the product's own synchronous CLI verbs.
/// They run as blocking children on a dedicated task because
/// `eggwork-server` must never enable Tokio process creation in production.
async fn operator_command(
    daemon: &Path,
    arguments: &[&str],
) -> Result<std::process::Output, String> {
    let daemon = daemon.to_path_buf();
    let label = arguments.join(" ");
    let invocation = arguments
        .iter()
        .map(|part| (*part).to_owned())
        .collect::<Vec<_>>();
    let config = std::env::var("EGGWORK_QUALIFY_CONFIG")
        .map_err(|_| "EGGWORK_QUALIFY_CONFIG is not set".to_owned())?;
    tokio::task::spawn_blocking(move || {
        Command::new(daemon)
            .args(&invocation)
            .arg("--config")
            .arg(&config)
            .stdin(Stdio::null())
            .output()
    })
    .await
    .map_err(|error| format!("waiting for the installed operator command failed: {error}"))?
    .map_err(|error| format!("running `eggworkd {label}` failed: {error}"))
}

/// The qualified properties of one installed release binary.
#[tokio::test]
#[ignore = "qualifies an installed release binary; run with --ignored"]
async fn installed_release_admits_execution_and_refuses_unsupported_isolation() -> Result<(), String>
{
    let subject = Subject::from_environment().expect("qualification subject");
    let mut facts: BTreeMap<String, Json> = BTreeMap::new();

    // The operator-facing configuration must be accepted by the installed
    // binary's own loader, not merely parse as JSON.
    let validate = operator_command(&subject.daemon, &["config", "validate"]).await?;
    assert!(
        validate.status.success(),
        "installed config validate failed: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let doctor = operator_command(&subject.daemon, &["doctor"]).await?;
    let doctor_report: Json = serde_json::from_slice(&doctor.stdout).expect("doctor reported JSON");
    assert_eq!(
        doctor_report.get("ready").and_then(Json::as_bool),
        Some(true),
        "installed doctor was not ready: {doctor_report}"
    );
    facts.insert(
        "doctor".to_owned(),
        json!(doctor_report.get("checks").cloned().unwrap_or(Json::Null)),
    );

    let node = start_installed_node(&subject, reserve_loopback_port())
        .await
        .expect("the installed node starts and reports ready");
    let client = client(&subject, &node).expect("the production client builds");

    let capabilities = client.capabilities().await.expect("capabilities");
    facts.insert(
        "capabilities".to_owned(),
        serde_json::to_value(&capabilities).unwrap_or(Json::Null),
    );
    let isolation_capable = capabilities
        .features
        .iter()
        .any(|feature| feature == "isolation.landlock.workspace-rw.v1");
    facts.insert("isolation_capable".to_owned(), json!(isolation_capable));

    // One authenticated loopback execution through the normal client path.
    let marker = subject.tls_dir.join("must-not-be-created");
    let success = qualify_execution(
        &client,
        marker_command(&marker, false),
        IsolationRequirement::None,
        &qualified_id("success"),
    )
    .await
    .expect("the installed node completes a bounded execution");
    assert_eq!(success["state"], json!("Succeeded"));
    assert_eq!(success["stdout"], json!("eggwork-qualification\n"));
    assert_eq!(
        success["cleanup_warning"],
        Json::Null,
        "the installed binary reported a cleanup warning: {success}"
    );
    facts.insert("execution".to_owned(), success);

    // Required filesystem isolation is admitted only where the node advertises
    // it, and where it is advertised it must actually be enforced. Everywhere
    // else the request must be refused *before* the target runs, which the
    // side-effect marker proves.
    if isolation_capable {
        let required = qualify_execution(
            &client,
            marker_command(&marker, false),
            IsolationRequirement::Required,
            &qualified_id("isolation"),
        )
        .await
        .expect("an advertised isolation profile is enforced");
        assert_eq!(
            required["state"],
            json!("Succeeded"),
            "required isolation did not execute where it is advertised: {required}"
        );
        assert_eq!(
            required["sandbox"]["status"],
            json!("applied"),
            "an advertised isolation profile was not applied: {required}"
        );
        // The sandbox is real: a write outside the workspace is denied.
        let denied = qualify_execution(
            &client,
            marker_command(&marker, true),
            IsolationRequirement::Required,
            &qualified_id("isolation-denied"),
        )
        .await
        .expect("the isolated execution itself runs");
        assert_ne!(
            denied["state"],
            json!("Succeeded"),
            "required isolation allowed a write outside the workspace: {denied}"
        );
        assert!(
            !marker.exists(),
            "required isolation did not deny a write outside the workspace"
        );
        facts.insert(
            "required_isolation".to_owned(),
            json!({
                "advertised": true,
                "applied": required["sandbox"],
                "outside_workspace_write_denied": true,
            }),
        );
    } else {
        let error = qualify_execution(
            &client,
            marker_command(&marker, true),
            IsolationRequirement::Required,
            &qualified_id("isolation-refused"),
        )
        .await
        .expect_err("unsupported required isolation must be refused");
        assert!(
            error.contains("capability") || error.contains("409"),
            "expected a capability_mismatch refusal, got {error:?}"
        );
        assert!(
            !marker.exists(),
            "the refused execution ran its target before admission"
        );
        facts.insert(
            "required_isolation".to_owned(),
            json!({"advertised": false, "refused": error, "marker_created": false}),
        );
    }

    // Drain is persistent and operator-controlled: it refuses new work until an
    // explicit clear, and the same node keeps serving afterwards.
    let drained = operator_command(&subject.daemon, &["drain"]).await?;
    assert!(drained.status.success());
    let refused = client
        .execute(
            &spec(marker_command(&marker, true), IsolationRequirement::None),
            &handle(&qualified_id("drained")),
        )
        .await;
    assert!(
        refused.is_err(),
        "a persistently draining node accepted a new execution"
    );
    let cleared = operator_command(&subject.daemon, &["undrain"]).await?;
    assert!(cleared.status.success());
    let admitted = qualify_execution(
        &client,
        marker_command(&marker, false),
        IsolationRequirement::None,
        &qualified_id("after-undrain"),
    )
    .await
    .expect("the node admits work again after an explicit undrain");
    assert_eq!(admitted["state"], json!("Succeeded"));
    facts.insert(
        "drain".to_owned(),
        json!({"refused_while_draining": true, "admitted_after_undrain": true}),
    );

    let report = json!({
        "schema_version": 1,
        "stage": format!("execution:{}", subject.platform),
        "installed_daemon": subject.daemon.display().to_string(),
        "facts": facts,
    });
    if let Some(path) = subject.receipt {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    Ok(())
}
