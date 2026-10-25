//! The hosted Windows service lifecycle (Operations M007).
//!
//! This suite mutates a real service control manager registration on a
//! hosted Windows runner. That is why it is `#[cfg(windows)]` and
//! `#[ignore]`d: an ordinary test run has no service control manager to
//! mutate, and a test that silently passes because its subject is missing is
//! worse than no test.
//!
//! ```text
//! EGGWORK_QUALIFY_ROOT      the installation root holding state/node.json and state/tls
//! EGGWORK_SERVICE_DAEMON    absolute path to the eggworkd under test
//! ```
//!
//! Everything the lifecycle touches goes through the product's own operator
//! CLI and Eggup's typed adapter. The one exception is the foreign-ownership
//! negative, which has to create a registration that is *not* Eggwork's, and
//! that is test orchestration: it creates the registration with the Windows
//! service tooling and then proves Eggwork refuses to replace, stop, or
//! remove it. Production code never shells out to a service manager.
//!
//! The suite is executed by the `windows-service-host` job in
//! `.github/workflows/ci.yml`, which builds the daemon from source and
//! materialises the repository's own qualification fixture.

#![cfg(windows)]

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use eggwork_client::NodeClient;
use eggwork_core::{
    CommandSpec, EnvironmentEntry, ExecutionGeneration, ExecutionHandle, ExecutionId,
    ExecutionSpec, ExecutionState, IsolationRequirement, LeaseId, NetworkRequirement, NodeId,
    OutputPolicy, Requirement, ResourceRequirements, StdinPolicy,
};
use serde_json::Value as Json;

/// How long the suite waits for the manager to reach a state.
const STATE_TIMEOUT: Duration = Duration::from_secs(90);

/// How long the suite waits for the authenticated node to answer.
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// The name the product registers its service under.
const SERVICE_ID: &str = "eggwork-node";

/// The subject of the lifecycle.
#[derive(Clone)]
struct Subject {
    daemon: PathBuf,
    config: PathBuf,
    tls: PathBuf,
}

impl Subject {
    fn from_env() -> Result<Self, String> {
        let root = PathBuf::from(
            std::env::var("EGGWORK_QUALIFY_ROOT")
                .map_err(|_| "EGGWORK_QUALIFY_ROOT must name the installation root".to_owned())?,
        );
        let daemon =
            PathBuf::from(std::env::var("EGGWORK_SERVICE_DAEMON").map_err(|_| {
                "EGGWORK_SERVICE_DAEMON must name the daemon under test".to_owned()
            })?);
        if !root.is_absolute() || !daemon.is_absolute() {
            return Err("the installation root and the daemon must be absolute paths".into());
        }
        Ok(Self {
            daemon,
            config: root.join("state").join("node.json"),
            tls: root.join("state").join("tls"),
        })
    }

    /// The loopback port the node's configuration binds.
    fn port(&self) -> Result<u16, String> {
        let text = std::fs::read_to_string(&self.config)
            .map_err(|error| format!("the node configuration is unreadable: {error}"))?;
        let config: Json = serde_json::from_str(&text)
            .map_err(|error| format!("the node configuration is not JSON: {error}"))?;
        let bind = config
            .get("bind")
            .and_then(Json::as_str)
            .ok_or_else(|| "the node configuration does not declare a bind address".to_owned())?;
        let port = bind
            .rsplit(':')
            .next()
            .and_then(|port| port.parse::<u16>().ok())
            .ok_or_else(|| format!("the bind address {bind:?} declares no port"))?;
        Ok(port)
    }

    /// The endpoint the fixture identity is valid for.
    fn endpoint(&self) -> Result<String, String> {
        Ok(format!("https://localhost:{}", self.port()?))
    }

    /// An authenticated client for the fixture identity.
    fn client(&self) -> Result<NodeClient, String> {
        NodeClient::from_pem_files(
            self.endpoint()?,
            self.tls.join("ca.pem"),
            self.tls.join("client.pem"),
            self.tls.join("client-key.pem"),
        )
        .map_err(|error| {
            format!("the production client could not use the fixture identity: {error}")
        })
    }
}

/// The registration's observed ownership and state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    ownership: String,
    state: String,
}

/// Run one operator CLI verb against the service.
fn service_verb(subject: &Subject, verb: &str) -> std::process::Output {
    Command::new(&subject.daemon)
        .arg("service")
        .arg(verb)
        .arg("--executable")
        .arg(&subject.daemon)
        .arg("--service-config")
        .arg(&subject.config)
        .arg("--config")
        .arg(&subject.config)
        .output()
        .unwrap_or_else(|error| panic!("running `eggworkd service {verb}` failed: {error}"))
}

/// Run one operator CLI verb against a *named* service configuration.
fn service_verb_with_config(
    subject: &Subject,
    verb: &str,
    service_config: &Path,
) -> std::process::Output {
    Command::new(&subject.daemon)
        .arg("service")
        .arg(verb)
        .arg("--executable")
        .arg(&subject.daemon)
        .arg("--service-config")
        .arg(service_config)
        .arg("--config")
        .arg(&subject.config)
        .output()
        .unwrap_or_else(|error| panic!("running `eggworkd service {verb}` failed: {error}"))
}

/// The observed ownership and state of the registration.
fn observe(subject: &Subject) -> Result<Observed, String> {
    let output = service_verb(subject, "status");
    let text = String::from_utf8_lossy(&output.stdout);
    let report: Json = serde_json::from_str(&text)
        .map_err(|error| format!("`service status` did not report JSON: {error}: {text}"))?;
    Ok(Observed {
        ownership: report
            .get("ownership")
            .and_then(Json::as_str)
            .ok_or_else(|| "`service status` did not report an ownership".to_owned())?
            .to_owned(),
        state: report
            .get("state")
            .and_then(Json::as_str)
            .ok_or_else(|| "`service status` did not report a state".to_owned())?
            .to_owned(),
    })
}

/// Wait for the registration to reach a state.
fn await_state(subject: &Subject, expected: &str) -> Result<Observed, String> {
    let deadline = Instant::now() + STATE_TIMEOUT;
    let mut last = String::new();
    while Instant::now() < deadline {
        let observed = observe(subject)?;
        last = format!("{} / {}", observed.ownership, observed.state);
        if observed.state == expected {
            return Ok(observed);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "the registration never reached {expected}; the last observation was {last}"
    ))
}

/// A registration Eggwork owns, removed when the test ends.
///
/// The removal runs on the failure path too: a failed assertion must not
/// leave a registration on the runner.
struct Registration {
    subject: Subject,
    installed: bool,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if !self.installed {
            return;
        }
        let _ = service_verb(&self.subject, "uninstall");
    }
}

/// Remove a registration the platform's own tooling created.
fn delete_registration() {
    let _ = Command::new("sc.exe")
        .arg("delete")
        .arg(SERVICE_ID)
        .output();
}

/// A registration that is not Eggwork's, removed when the test ends.
struct ForeignRegistration;

impl Drop for ForeignRegistration {
    fn drop(&mut self) {
        delete_registration();
    }
}

/// Create a registration that is not Eggwork's.
///
/// The registration uses the *same name* and the same argv shape, but a
/// different executable — which is what "same-name, different-executable"
/// means in Eggup's own ownership model. The binary is a real copy, so the
/// registration is plausible rather than a dangling path.
fn create_foreign_registration(subject: &Subject) -> Result<ForeignRegistration, String> {
    delete_registration();
    let foreign_dir = subject
        .config
        .parent()
        .ok_or_else(|| "the installation root has no parent".to_owned())?
        .join("foreign");
    std::fs::create_dir_all(&foreign_dir).map_err(|error| {
        format!("the foreign installation directory could not be created: {error}")
    })?;
    let foreign_executable = foreign_dir.join("foreign-eggworkd.exe");
    std::fs::copy(&subject.daemon, &foreign_executable)
        .map_err(|error| format!("the foreign daemon could not be staged: {error}"))?;

    // The command line is quoted the way the manager quotes it: one quoted
    // token per argument, so the registration is parseable and its
    // difference is a real difference rather than a quoting artefact.
    let command_line = format!(
        "\"{}\" \"service-host\" \"--config\" \"{}\"",
        foreign_executable.display(),
        subject.config.display()
    );
    let output = Command::new("sc.exe")
        .arg("create")
        .arg(SERVICE_ID)
        .arg(format!("binPath= {command_line}"))
        .arg("start=")
        .arg("disabled")
        .output()
        .map_err(|error| format!("the platform's service tooling could not be run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "the platform's service tooling refused the foreign registration: {}",
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    Ok(ForeignRegistration)
}

/// The environment a qualified target declares for its child.
///
/// A Windows process started with only `PATH` cannot initialise: the loader
/// and `cmd.exe` both need the system directories.
fn qualified_environment() -> Vec<EnvironmentEntry> {
    let mut environment = Vec::new();
    for name in ["PATH", "SystemRoot", "windir", "ComSpec", "TEMP", "TMP"] {
        let Some(value) = std::env::var_os(name) else {
            continue;
        };
        let Some(value) = value.to_str() else {
            continue;
        };
        environment.push(EnvironmentEntry::new(name, value).expect("windows environment entry"));
    }
    environment
}

/// A command that stays alive long enough to be stopped.
fn long_running_command() -> Vec<String> {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned());
    vec![
        shell,
        "/C".to_owned(),
        "ping -n 120 127.0.0.1 > nul".to_owned(),
    ]
}

/// A per-run identifier, so the retained record cannot collide.
fn run_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}-{:x}", std::process::id())
}

/// The durable record of an execution, read through the operator CLI.
///
/// The node is stopped by the time this is read, so the record is read from
/// the journal rather than from a live node.
fn durable_execution_state(subject: &Subject, id: &str) -> Result<ExecutionState, String> {
    let output = Command::new(&subject.daemon)
        .arg("executions")
        .arg("show")
        .arg(id)
        .arg("--config")
        .arg(&subject.config)
        .output()
        .map_err(|error| format!("the operator CLI could not be run: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let report: Json = serde_json::from_str(&text)
        .map_err(|error| format!("`executions show` did not report JSON: {error}: {text}"))?;
    let state = report
        .get("state")
        .ok_or_else(|| "`executions show` did not report a state".to_owned())?;
    serde_json::from_value(state.clone())
        .map_err(|error| format!("`executions show` reported an unusable state: {error}"))
}

/// The full hosted lifecycle, from a source-built daemon.
#[tokio::test]
#[ignore = "mutates a real service control manager registration"]
async fn the_windows_service_lifecycle_is_owned_and_reachable() -> Result<(), String> {
    let subject = Subject::from_env()?;
    let mut registration = Registration {
        subject: subject.clone(),
        installed: false,
    };
    println!("RECEIPT: daemon {}", subject.daemon.display());
    println!("RECEIPT: config {}", subject.config.display());

    // 1. Install.
    let install = service_verb(&subject, "install");
    assert!(
        install.status.success(),
        "`service install` failed: {}",
        String::from_utf8_lossy(&install.stdout)
    );
    registration.installed = true;
    let observed = observe(&subject)?;
    assert_eq!(
        observed.ownership, "Owned",
        "the registration Eggwork just made must be owned"
    );
    println!(
        "RECEIPT: install completed; ownership={} state={}",
        observed.ownership, observed.state
    );

    // 2. Start, and wait until the manager reports it running.
    let start = service_verb(&subject, "start");
    assert!(
        start.status.success(),
        "`service start` failed: {}",
        String::from_utf8_lossy(&start.stdout)
    );
    let observed = await_state(&subject, "Running")?;
    assert_eq!(observed.ownership, "Owned");
    println!(
        "RECEIPT: start completed; ownership={} state={}",
        observed.ownership, observed.state
    );

    // 3. The service says Running *and* the node answers an authenticated
    // request. The manager's state alone would not prove the node is
    // serving.
    let client = subject.client()?;
    let capabilities = tokio::time::timeout(READINESS_TIMEOUT, client.capabilities())
        .await
        .map_err(|_| "the service-hosted node did not answer a capabilities request".to_owned())?
        .map_err(|error| {
            format!("the service-hosted node refused a capabilities request: {error}")
        })?;
    assert!(
        !capabilities.features.is_empty(),
        "the service-hosted node advertised no capability at all"
    );
    let status = tokio::time::timeout(READINESS_TIMEOUT, client.status())
        .await
        .map_err(|_| "the service-hosted node did not answer a status request".to_owned())?
        .map_err(|error| format!("the service-hosted node refused a status request: {error}"))?;
    assert_eq!(
        status.node_id,
        NodeId::new("qualification-node").expect("the fixture node id"),
        "the authenticated request reached a different node"
    );
    println!(
        "RECEIPT: the service-hosted node answered an authenticated capabilities and status request at {}",
        subject.endpoint()?
    );

    // 4. A stop converges an active execution before the process exits.
    let execution_id = format!("service-host-{}", run_id());
    let identifier = ExecutionId::new(&execution_id).map_err(|_| "the execution id is unusable")?;
    let handle = ExecutionHandle {
        execution_id: identifier.clone(),
        generation: ExecutionGeneration::new(1).expect("generation"),
        lease_id: LeaseId::new(format!("lease-{execution_id}")).expect("lease"),
    };
    let spec = ExecutionSpec {
        schema_version: 1,
        command: CommandSpec {
            argv: long_running_command(),
            cwd: None,
            environment: qualified_environment(),
            stdin: StdinPolicy::Null,
            timeout_millis: 120_000,
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
    };
    let stream = tokio::time::timeout(READINESS_TIMEOUT, client.execute(&spec, &handle))
        .await
        .map_err(|_| "the service-hosted node did not accept the execution".to_owned())?
        .map_err(|error| format!("the service-hosted node refused the execution: {error}"))?;
    // The stream is drained in the background so the execution keeps its
    // lease while the service is stopped.
    let drain = tokio::spawn(async move {
        use futures_util::StreamExt;
        let mut events = Box::pin(stream.into_events());
        while let Some(event) = events.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    // 5. Stop, and prove the registration is stopped.
    let stop = service_verb(&subject, "stop");
    assert!(
        stop.status.success(),
        "`service stop` failed: {}",
        String::from_utf8_lossy(&stop.stdout)
    );
    let observed = await_state(&subject, "Stopped")?;
    assert_eq!(observed.ownership, "Owned");
    println!(
        "RECEIPT: stop completed; ownership={} state={}",
        observed.ownership, observed.state
    );

    // 6. The execution converged: the node reached Stopped only after its
    // active executions finished, so the durable record is already terminal.
    let state = durable_execution_state(&subject, &execution_id)?;
    assert!(
        state.is_terminal(),
        "the execution was still {state:?} after the service stopped"
    );
    println!("RECEIPT: the active execution converged to {state:?} before the process exited");
    drain.abort();

    // 7. Start again, and prove it runs.
    println!("RECEIPT: the second start begins");
    let start = service_verb(&subject, "start");
    assert!(
        start.status.success(),
        "the second `service start` failed: {}",
        String::from_utf8_lossy(&start.stdout)
    );
    let observed = await_state(&subject, "Running")?;
    assert_eq!(observed.ownership, "Owned");

    // 8. Restart, and prove it runs.
    println!("RECEIPT: the restart begins");
    let restart = service_verb(&subject, "restart");
    assert!(
        restart.status.success(),
        "`service restart` failed: {}",
        String::from_utf8_lossy(&restart.stdout)
    );
    let observed = await_state(&subject, "Running")?;
    assert_eq!(observed.ownership, "Owned");

    // 9. Uninstall, and prove the registration is gone.
    let uninstall = service_verb(&subject, "uninstall");
    assert!(
        uninstall.status.success(),
        "`service uninstall` failed: {}",
        String::from_utf8_lossy(&uninstall.stdout)
    );
    registration.installed = false;
    let observed = observe(&subject)?;
    assert_ne!(
        observed.ownership, "Owned",
        "the registration survived its own removal"
    );
    println!(
        "RECEIPT: uninstall completed; ownership={} state={}",
        observed.ownership, observed.state
    );

    // 10. A registration that is not Eggwork's is not mutated.
    //
    // The property under test is that Eggwork refuses to touch it. The
    // classification itself — `Foreign` for a registration that provably
    // belongs to another deployment, `Unknown` for one whose ownership
    // cannot be proven — is owned by Eggup and is pinned by the service
    // identity contract; here the requirement is that Eggwork does not
    // claim it and does not mutate it.
    let _foreign = create_foreign_registration(&subject)?;
    let observed = observe(&subject)?;
    assert_ne!(
        observed.ownership, "Owned",
        "a same-name registration with a different identity must not be owned"
    );
    for verb in ["stop", "uninstall", "start"] {
        let output = service_verb(&subject, verb);
        assert!(
            !output.status.success(),
            "`service {verb}` must refuse a registration Eggwork does not own"
        );
    }
    delete_registration();
    println!("RECEIPT: a same-name registration Eggwork does not own was not mutated");

    // 11. A node that cannot start never reports Running.
    let unavailable = subject.config.with_extension("unavailable.json");
    std::fs::write(&unavailable, b"{\"schema_version\":1}")
        .map_err(|error| format!("the unusable configuration could not be written: {error}"))?;
    let install = service_verb_with_config(&subject, "install", &unavailable);
    assert!(
        install.status.success(),
        "installing an unusable registration failed: {}",
        String::from_utf8_lossy(&install.stdout)
    );
    let start = service_verb_with_config(&subject, "start", &unavailable);
    assert!(
        !start.status.success(),
        "a service whose configuration cannot be loaded must not start"
    );
    let observed = observe(&subject).unwrap_or(Observed {
        ownership: "Unknown".to_owned(),
        state: "Unknown".to_owned(),
    });
    assert_ne!(
        observed.state, "Running",
        "a service whose configuration cannot be loaded must never report Running"
    );
    println!(
        "RECEIPT: a node whose configuration cannot be loaded never reported Running; \
         ownership={} state={}",
        observed.ownership, observed.state
    );
    let _ = service_verb_with_config(&subject, "uninstall", &unavailable);
    delete_registration();

    Ok(())
}
