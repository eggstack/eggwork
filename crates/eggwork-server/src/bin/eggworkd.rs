use eggwork_server::deployment;
use eggwork_server::operations::{
    OperatorConfig, collect_garbage, doctor, execution_page, execution_show, inspect_artifact,
    inspect_blob, inspect_workspace, is_persistently_draining, metrics_snapshot,
    observed_permit_count, set_persistent_drain, storage_summary,
};
use eggwork_server::service_host::{LifecycleEdge, serve_until_stopped};
use serde::Serialize;
use std::{env, path::PathBuf, process::ExitCode};

/// The process modes the daemon has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The ordinary operator CLI.
    Cli,
    /// The Windows service control manager entry.
    ServiceHost,
}

/// Choose the process mode from the command line.
///
/// The mode is explicit. It is never inferred from the parent process, the
/// session, the environment, or a dispatcher error, and foreground `run`
/// keeps its existing meaning: only the literal `service-host` verb selects
/// the dispatcher, and only on Windows.
fn mode_for(args: &[String]) -> Mode {
    match args.first().map(String::as_str) {
        Some("service-host") => Mode::ServiceHost,
        _ => Mode::Cli,
    }
}

/// The node lifecycle, shared by the foreground and the service-hosted paths.
///
/// Both paths run the same function, so they converge the same way: stop
/// admission, cancel and converge the active executions, shut the listener
/// down, and **wait** for the listener and the executions to finish before
/// the process exits. Foreground `run` used to call `shutdown()` without
/// `wait()`, so a foreground stop could return while an execution was still
/// converging.
///
/// The foreground path resolves the stop edge on Ctrl-C, and the
/// service-hosted path resolves it when the service control manager
/// delivers `STOP` or `SHUTDOWN`.
fn main() -> ExitCode {
    // The mode is chosen **before** an asynchronous runtime exists.
    //
    // The Windows service control dispatcher owns the calling thread until the
    // service exits, so it cannot run inside a Tokio runtime that the
    // dispatcher thread is not part of. The service-hosted path therefore
    // enters its own runtime from the service-main callback, and only the
    // ordinary CLI builds one here.
    let args: Vec<String> = env::args().skip(1).collect();
    match mode_for(&args) {
        Mode::ServiceHost => service_host_entry(),
        Mode::Cli => cli_entry(args),
    }
}

/// The ordinary CLI path.
fn cli_entry(args: Vec<String>) -> ExitCode {
    // The operator CLI validates TLS server identity locally (config
    // validate/doctor/status/deployment/service) and serves it (run). No
    // global CryptoProvider is installed by the libraries, so install the
    // process default once; without it every TLS config reports invalid.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("eggworkd: could not build the async runtime: {error}");
            return ExitCode::from(2);
        }
    };
    match runtime.block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("eggworkd: {error}");
            ExitCode::from(2)
        }
    }
}

/// The Windows service host entry.
///
/// The service control manager launches the registered daemon with the argv
/// `eggworkd service-host --config <absolute-path>`, so this is the mode the
/// manager starts. The mode is never inferred from the parent process, the
/// session, the environment, or a dispatcher error.
#[cfg(windows)]
fn service_host_entry() -> ExitCode {
    eggwork_server::service_host::windows::entry()
}

/// The service host exists only on Windows.
///
/// The dispatcher is a Windows service control manager interface; on every
/// other platform the mode is refused before it can do anything.
#[cfg(not(windows))]
fn service_host_entry() -> ExitCode {
    eprintln!("eggworkd: the service host is only available on Windows");
    ExitCode::from(2)
}

async fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().cloned() else {
        return Err(usage().into());
    };
    if command == "version" {
        return output(&serde_json::json!({"version": env!("CARGO_PKG_VERSION")}));
    }
    let config_index = args
        .iter()
        .position(|arg| arg == "--config")
        .ok_or_else(|| usage().to_owned())?;
    let config_path = PathBuf::from(
        args.get(config_index + 1)
            .ok_or_else(|| usage().to_owned())?,
    );
    let mut args = args;
    args.drain(config_index..=config_index + 1);
    let index = 1;
    // A configuration fault renders as `[rule] item: action`. The rule is a
    // stable identifier an operator can grep for, the item is the file or
    // path that failed — named by role alone when it is secret-bearing — and
    // the action says what to change. No other error text is interpolated:
    // the library's other errors are fixed phrases, and a configuration fault
    // never carries the underlying `io` or `serde_json` message.
    let config = OperatorConfig::load(&config_path).map_err(|error| error.to_string())?;
    match command.as_str() {
        "config" => match args.get(index).map(String::as_str) {
            Some("validate") => {
                let report = doctor(&config).await;
                let valid = config.validate().is_ok() && report.ready;
                output(&serde_json::json!({"valid": valid, "doctor": report}))?;
                if valid {
                    Ok(())
                } else {
                    Err("configuration checks failed".into())
                }
            }
            Some("print") => {
                println!(
                    "{}",
                    config.redacted_json().map_err(|error| error.to_string())?
                );
                Ok(())
            }
            _ => Err(usage().into()),
        },
        "doctor" => {
            let report = doctor(&config).await;
            output(&report)?;
            if report.ready {
                Ok(())
            } else {
                Err("doctor found an unavailable or invalid check".into())
            }
        }
        "status" => {
            // `doctor` runs `config.validate()` — which builds a full
            // `TlsServerConfig` from the private key — and we embed its report in
            // the output below. Validating separately first meant doing that work
            // twice on every single `status` invocation, so ask the report we are
            // going to print anyway.
            let report = doctor(&config).await;
            if !report.configuration_ok() {
                // The same rule, item, and action every other surface reports.
                return Err(report
                    .configuration_detail()
                    .unwrap_or("configuration is invalid")
                    .to_owned());
            }
            let page = execution_page(&config, 1, 0)
                .await
                .map_err(|_| "execution state is unavailable".to_owned())?;
            let metrics = metrics_snapshot(&config)
                .map_err(|_| "operator metrics are unavailable".to_owned())?;
            // The two execution counts are different facts and are reported
            // under different names: `execution_permits_held` is how many
            // execution permits the running node holds, and
            // `non_terminal_executions` is how many durable rows are still in
            // a non-terminal state. An execution whose process died keeps its
            // row long after its permit was released, so the two disagree on
            // exactly the cases an operator is investigating.
            output(
                &serde_json::json!({"schema_version":1,"node_id":config.node_id,"draining":is_persistently_draining(&config.drain_path()),"execution_permits_held":observed_permit_count(&config),"non_terminal_executions":page.non_terminal_executions,"max_active_executions":config.max_active_executions,"execution_records":page.total,"execution_states":page.states,"stdout_bytes":page.stdout_bytes,"stderr_bytes":page.stderr_bytes,"cleanup_failures":page.cleanup_failures,"metrics":metrics,"doctor":report}),
            )
        }
        "drain" | "undrain" => {
            config.validate().map_err(|error| error.to_string())?;
            let draining = command == "drain";
            set_persistent_drain(&config.drain_path(), draining)
                .map_err(|_| "drain state could not be updated".to_owned())?;
            output(&serde_json::json!({"draining":draining,"persistent":true}))
        }
        "executions" => {
            config.validate().map_err(|error| error.to_string())?;
            match args.get(index).map(String::as_str) {
                Some("list") => {
                    let options = &args[index + 1..];
                    let limit = parse_option(options, "--limit", 50usize)?.clamp(1, 200);
                    let offset = parse_option(options, "--offset", 0usize)?.min(2048);
                    output(
                        &execution_page(&config, limit, offset)
                            .await
                            .map_err(|_| "execution state is unavailable".to_owned())?,
                    )
                }
                Some("show") => {
                    let id = args.get(index + 1).ok_or_else(|| usage().to_owned())?;
                    let generation = parse_option(&args[index + 2..], "--generation", 0u64)?;
                    let generation = (generation != 0).then_some(generation);
                    output(
                        &execution_show(&config, id, generation).await.map_err(|_| {
                            "execution was not found or could not be read".to_owned()
                        })?,
                    )
                }
                _ => Err(usage().into()),
            }
        }
        "metrics" => {
            config.validate().map_err(|error| error.to_string())?;
            output(
                &metrics_snapshot(&config)
                    .map_err(|_| "operator metrics are unavailable".to_owned())?,
            )
        }
        "storage" => {
            config.validate().map_err(|error| error.to_string())?;
            match args.get(index).map(String::as_str) {
                Some("summary") => output(
                    &storage_summary(&config)
                        .map_err(|_| "storage summary is unavailable".to_owned())?,
                ),
                _ => Err(usage().into()),
            }
        }
        "inspect" => {
            config.validate().map_err(|error| error.to_string())?;
            let kind = args
                .get(index)
                .map(String::as_str)
                .ok_or_else(|| usage().to_owned())?;
            let id = args.get(index + 1).ok_or_else(|| usage().to_owned())?;
            let result = match kind {
                "blob" => inspect_blob(&config, id),
                "workspace" => inspect_workspace(&config, id),
                "artifact" => inspect_artifact(&config, id),
                _ => return Err(usage().into()),
            }
            .map_err(|_| "resource was not found or could not be inspected".to_owned())?;
            output(&result)
        }
        "gc" => {
            config.validate().map_err(|error| error.to_string())?;
            let dry_run = !args[index..].iter().any(|arg| arg == "--apply");
            let limit = parse_option(&args[index..], "--limit", 128usize)?.clamp(1, 1024);
            output(
                &collect_garbage(&config, dry_run, limit)
                    .await
                    .map_err(|_| "bounded garbage collection failed".to_owned())?,
            )
        }
        "run" => {
            config.validate().map_err(|error| error.to_string())?;
            // The foreground path and the service-hosted path run the same
            // lifecycle, so a Ctrl-C and a service stop converge the same
            // way: admission stops, the active executions are cancelled and
            // waited for, and the listener closes.
            let (stop, mut stopped) = tokio::sync::watch::channel(false);
            let observed = stopped.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                let _ = stop.send(true);
            });
            let node_id = config.node_id.clone();
            serve_until_stopped(
                &config,
                || {
                    let requested = observed.borrow();
                    *requested
                },
                async move {
                    loop {
                        let requested = {
                            let current = stopped.borrow();
                            *current
                        };
                        if requested {
                            break;
                        }
                        if stopped.changed().await.is_err() {
                            break;
                        }
                    }
                },
                move |edge| {
                    if let LifecycleEdge::Ready(local_addr) = edge {
                        println!(
                            "{}",
                            serde_json::json!({"ready":true,"node_id":node_id,"local_addr":local_addr})
                        );
                    }
                },
            )
            .await
            .map_err(|error| error.to_string())
        }
        "deployment" => {
            config.validate().map_err(|error| error.to_string())?;
            match args.get(index).map(String::as_str) {
                Some("status") => {
                    let helper_required = config.sandbox_helper.is_some();
                    let status = deployment::DeploymentStatus::collect(
                        config.sandbox_helper.as_deref(),
                        helper_required,
                    );
                    let facts = deployment::platform_support()
                        .map(|facts| {
                            serde_json::json!({"os": facts.os, "systemd_available": facts.systemd_available, "launchd_available": facts.launchd_available, "crontab_available": facts.crontab_available, "candidates": deployment::candidate_managers_for(&facts).iter().map(|c| format!("{c:?}")).collect::<Vec<_>>()})
                        })
                        .unwrap_or(serde_json::json!({"unavailable": true}));
                    let executable = std::env::current_exe()
                        .ok()
                        .and_then(|path| path.canonicalize().ok())
                        .map(|path| path.display().to_string());
                    output(
                        &serde_json::json!({"schema_version": 1, "deployment": status, "service_id": deployment::SERVICE_ID, "executable": executable, "platform": facts}),
                    )
                }
                Some("apply") => deployment_apply(&args[index..], &config),
                _ => Err(usage().into()),
            }
        }
        "service" => {
            config.validate().map_err(|error| error.to_string())?;
            service_command(&args[index..]).await
        }
        _ => Err(usage().into()),
    }
}

async fn service_command(args: &[String]) -> Result<(), String> {
    let verb = args.first().map(String::as_str).ok_or_else(usage)?;
    let executable = match args.iter().position(|arg| arg == "--executable") {
        Some(position) => std::path::PathBuf::from(
            args.get(position + 1)
                .ok_or_else(|| "--executable requires a value".to_owned())?,
        ),
        None => std::env::current_exe()
            .ok()
            .and_then(|path| path.canonicalize().ok())
            .ok_or_else(|| "service commands require --executable <absolute-path>".to_owned())?,
    };
    if !executable.is_absolute() {
        return Err("service executable must be absolute".into());
    }
    let config_path = match args.iter().position(|arg| arg == "--service-config") {
        Some(position) => std::path::PathBuf::from(
            args.get(position + 1)
                .ok_or_else(|| "--service-config requires a value".to_owned())?,
        ),
        // Fail closed: the service identity must name the exact registered
        // config path, so it is never guessed from the operator config.
        None => return Err("service commands require --service-config <absolute-path>".into()),
    };
    if !config_path.is_absolute() {
        return Err("service config path must be absolute".into());
    }
    let spec = deployment::service_spec_for_install(&executable, Some(&config_path))
        .map_err(|error| error.to_string())?;
    match verb {
        "spec" => output(
            &serde_json::json!({"schema_version": 1, "service_id": spec.id().as_str(), "executable": spec.executable().display().to_string(), "args": spec.args(), "config": spec.config().map(|p| p.display().to_string())}),
        ),
        "status" | "install" | "uninstall" | "start" | "stop" | "restart" => {
            service_mutation(&spec, args, verb).await
        }
        _ => Err(usage().into()),
    }
}

/// Destructive service lifecycle through Eggup adapters only.
///
/// Linux systemd is the qualified path. Other platforms receive a structured
/// diagnostic instead of a parallel manager implementation.
async fn service_mutation(
    spec: &eggup_service::ServiceSpec,
    args: &[String],
    verb: &str,
) -> Result<(), String> {
    let mut manager = product_service_manager(spec, args)?;
    apply_service_operation(&mut *manager, spec, verb, service_backend())
}

fn product_service_manager(
    spec: &eggup_service::ServiceSpec,
    args: &[String],
) -> Result<Box<dyn eggup_service::ServiceManager>, String> {
    #[cfg(target_os = "macos")]
    {
        let plist_path = required_path(args, "--plist-path")?;
        let domain_text = option_value(args, "--launchd-domain")
            .ok_or_else(|| "service lifecycle requires --launchd-domain user|system".to_owned())?;
        let (domain, default_target) = match domain_text {
            "user" => (eggup_service::LaunchdDomain::UserAgent, None),
            "system" => (eggup_service::LaunchdDomain::SystemDaemon, Some("system")),
            _ => return Err("--launchd-domain must be user or system".into()),
        };
        let target = match option_value(args, "--launchd-target") {
            Some(target) => target.to_owned(),
            None if default_target.is_some() => default_target.unwrap().to_owned(),
            None => return Err("user launchd requires --launchd-target gui/<uid>".into()),
        };
        let manager = deployment::launchd_manager(
            domain,
            &target,
            &plist_path,
            spec.executable(),
            spec.config()
                .ok_or_else(|| "service spec requires a config path".to_owned())?,
            args.iter().any(|arg| arg == "--bootstrap-on-install"),
            std::time::Duration::from_secs(60),
        )
        .map_err(|error| error.to_string())?;
        // The macOS branch is the branch's tail expression on macOS, so a
        // `return` here is a needless return that `clippy -D warnings` rejects
        // on that target. It does not fire on Linux or Windows, where the
        // branch is compiled out.
        Ok(Box::new(manager))
    }
    // Windows service management goes through Eggup's typed SCM adapter,
    // which owns the registration and the manager operations. Eggwork hosts
    // the service process — `eggworkd service-host` joins the dispatcher —
    // and never registers or manages it itself.
    //
    // The adapter is only reachable now that the daemon can actually answer
    // a start request: registering a service whose binary never calls
    // `StartServiceCtrlDispatcherW` produces a registration that can never
    // reach `Running`, which is the error 1053 the previous milestone
    // recorded. The host is the reason this branch is no longer a refusal.
    #[cfg(windows)]
    {
        // The spec is the identity Eggup mutates; the SCM adapter takes the
        // registration policy instead, so the spec is not read here.
        let _ = spec;
        let start_type =
            deployment::parse_windows_start_type(option_value(args, "--windows-start-type"))
                .map_err(|error| error.to_string())?;
        let manager =
            deployment::windows_scm_manager(start_type, std::time::Duration::from_secs(60))
                .map_err(|error| error.to_string())?;
        Ok(Box::new(manager))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    return Err("service management is unsupported on this platform".into());
    #[cfg(target_os = "linux")]
    {
        let unit_path = std::path::PathBuf::from(
            args.iter()
                .position(|arg| arg == "--unit-path")
                .and_then(|position| args.get(position + 1))
                .ok_or_else(|| {
                    "service lifecycle requires --unit-path <absolute-path>".to_owned()
                })?,
        );
        let scope_text = args
            .iter()
            .position(|arg| arg == "--scope")
            .and_then(|position| args.get(position + 1))
            .map(String::as_str)
            .unwrap_or("system");
        let scope =
            deployment::parse_systemd_scope(scope_text).map_err(|error| error.to_string())?;
        let unit_name = unit_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "unit path must name a *.service file".to_owned())?
            .to_owned();
        let enable = args.iter().any(|arg| arg == "--enable");
        let reload = !args.iter().any(|arg| arg == "--no-reload");
        let manager = deployment::systemd_manager(
            &unit_name,
            scope,
            &unit_path,
            spec.executable(),
            spec.config()
                .ok_or_else(|| "service spec requires a config path".to_owned())?,
            enable,
            reload,
            std::time::Duration::from_secs(60),
        )
        .map_err(|error| error.to_string())?;
        Ok(Box::new(manager))
    }
}

fn service_backend() -> &'static str {
    if cfg!(target_os = "linux") {
        "systemd"
    } else if cfg!(target_os = "macos") {
        "launchd"
    } else if cfg!(windows) {
        // The backend names the manager Eggup drives. On Windows that is the
        // service control manager, reached through Eggup's typed adapter.
        "windows-scm"
    } else {
        "unsupported"
    }
}

fn deployment_apply(
    args: &[String],
    config: &eggwork_server::operations::OperatorConfig,
) -> Result<(), String> {
    let installation_root = absolute_option(args, "--installation-root")?;
    let release = required_option(args, "--release")?;
    let daemon_candidate = absolute_option(args, "--daemon")?;
    let service_config = absolute_option(args, "--service-config")?;
    let mut sources = vec![deployment::CandidateSource {
        member: deployment::MEMBER_DAEMON.to_owned(),
        source: daemon_candidate,
        destination: deployment::DEST_DAEMON.to_owned(),
    }];
    let helper_candidate = option_value(args, "--helper").map(std::path::PathBuf::from);
    if std::env::consts::OS == "linux" {
        let helper = helper_candidate.ok_or_else(|| {
            "Linux release apply requires --helper <absolute-candidate>".to_owned()
        })?;
        if !helper.is_absolute() {
            return Err("--helper must be an absolute path".into());
        }
        sources.push(deployment::CandidateSource {
            member: deployment::MEMBER_HELPER.to_owned(),
            source: helper,
            destination: deployment::DEST_HELPER.to_owned(),
        });
    } else if helper_candidate.is_some() {
        return Err("--helper is only valid for Linux release bundles".into());
    }

    let previous_daemon = option_value(args, "--previous-daemon-sha256");
    let previous_helper = option_value(args, "--previous-helper-sha256");
    if (previous_daemon.is_some() || previous_helper.is_some())
        && (previous_daemon.is_none()
            || (std::env::consts::OS == "linux" && previous_helper.is_none()))
    {
        return Err("replacement requires previous SHA-256 values for every release member".into());
    }
    let mut expected = Vec::new();
    if let Some(digest) = previous_daemon {
        expected.push((
            eggup_core::MemberId::new(deployment::MEMBER_DAEMON.to_owned())
                .map_err(|e| e.to_string())?,
            parse_sha256(digest)?,
        ));
    }
    if let Some(digest) = previous_helper {
        expected.push((
            eggup_core::MemberId::new(deployment::MEMBER_HELPER.to_owned())
                .map_err(|e| e.to_string())?,
            parse_sha256(digest)?,
        ));
    }
    let verifier = eggup_core::ExactDigestVerifier::new(expected);
    let executable = installation_root.join(deployment::DEST_DAEMON);
    let spec = deployment::service_spec_for_install(&executable, Some(&service_config))
        .map_err(|error| error.to_string())?;
    let mut manager = product_service_manager(&spec, args)?;
    let drain_marker = config.drain_path();
    let installed_helper = installation_root.join(deployment::DEST_HELPER);
    let require_helper = std::env::consts::OS == "linux";
    let receipt = deployment::orchestrate_update_with_lifecycle_budgeted(
        &mut *manager,
        deployment::UpdateRequest {
            spec: &spec,
            installation_root: &installation_root,
            release,
            sources: &sources,
            drain_marker: Some(&drain_marker),
            policy: deployment::UpdatePolicy {
                force: args.iter().any(|arg| arg == "--force"),
                ..deployment::UpdatePolicy::default()
            },
        },
        &verifier,
        || eggwork_server::operations::active_execution_count(config),
        move |remaining| {
            // One deadline for the whole post-commit check. `remaining` is a
            // *duration* measured when Eggup computed it, not a deadline, so
            // deriving `now + remaining` again further down — after the daemon
            // probe has already been allowed up to 5 s of it — would hand this
            // phase the entire remaining allowance a second time. Computing the
            // deadline once here makes the Linux helper check below spend what
            // is genuinely left, which is what `deployment-lifecycle.md` and
            // `operations-cli.md` both describe.
            let deadline = std::time::Instant::now() + remaining;
            let timeout = std::cmp::min(remaining, std::time::Duration::from_secs(5));
            if timeout.is_zero() {
                return Err("post-install check budget exhausted".into());
            }
            let output = eggup_core::run_bounded(
                &eggup_core::CommandSpec::new(executable.clone())
                    .arg("version")
                    .timeout(timeout)
                    .max_output_bytes(deployment::MAX_VERSION_PROBE_BYTES),
            )
            .map_err(|_| "installed daemon version probe failed".to_owned())?;
            let expected = env!("CARGO_PKG_VERSION");
            let reported = deployment::check_installed_daemon_version(output.stdout(), expected);
            if !output.success() {
                return Err("installed daemon version probe did not succeed".into());
            }
            if !reported.matched() {
                return Err(match &reported {
                    deployment::InstalledVersionCheck::Mismatch { reported } => {
                        format!("installed daemon reports version {reported}, not this release")
                    }
                    deployment::InstalledVersionCheck::Unreadable { reason } => {
                        format!("installed daemon version report is unusable: {reason}")
                    }
                    deployment::InstalledVersionCheck::Matched { .. } => {
                        unreachable!("matched version is not a failure")
                    }
                });
            }
            #[cfg(target_os = "linux")]
            if require_helper {
                // Only the Linux path still spends the remaining budget on the
                // helper compatibility check, so the budget is derived here
                // rather than being an unused binding on every other target.
                let helper_budget = deadline.saturating_duration_since(std::time::Instant::now());
                if !deployment::check_helper_compatibility_with_timeout(
                    Some(&installed_helper),
                    env!("CARGO_PKG_VERSION"),
                    helper_budget,
                )
                .compatible()
                {
                    return Err(
                        "installed sandbox helper is untrusted or version-incoherent".into(),
                    );
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = (&installed_helper, require_helper, &deadline);
            Ok(())
        },
    )
    .map_err(|error| error.to_string())?;
    let final_snapshot = receipt.final_snapshot.as_ref();
    let failure = |report: Option<&eggup_core::FailureReport>| {
        report.map(|report| {
            serde_json::json!({
                "phase": format!("{:?}", report.phase()),
                "category": format!("{:?}", report.category()),
                "detail": report.detail(),
            })
        })
    };
    output(&serde_json::json!({
        "schema_version": 1,
        "release_id": release,
        "artifact_disposition": format!("{:?}", receipt.transaction.disposition()),
        "manual_artifact_recovery_required": receipt.manual_artifact_recovery_required(),
        "lifecycle_restoration": format!("{:?}", receipt.restoration),
        "final_ownership": final_snapshot.map(|snapshot| format!("{:?}", snapshot.ownership)),
        "final_state": final_snapshot.map(|snapshot| format!("{:?}", snapshot.state)),
        "drain_remains_active": true,
        // A failed post-install check must be attributable, not just visible as
        // a rolled-back disposition. Detail is already bounded by Eggup's
        // failure report, so this never emits unbounded child output.
        "artifact_failure": failure(receipt.transaction.failure()),
        "post_commit_failure": failure(receipt.transaction.post_commit_failure()),
        "rollback_failure": failure(receipt.transaction.rollback_failure()),
    }))
}

fn required_option<'a>(args: &'a [String], name: &str) -> Result<&'a str, String> {
    option_value(args, name).ok_or_else(|| format!("deployment apply requires {name}"))
}

fn absolute_option(args: &[String], name: &str) -> Result<std::path::PathBuf, String> {
    let path = std::path::PathBuf::from(required_option(args, name)?);
    if !path.is_absolute() {
        return Err(format!("{name} must be an absolute path"));
    }
    Ok(path)
}

fn parse_sha256(text: &str) -> Result<[u8; 32], String> {
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("previous SHA-256 value must contain exactly 64 hexadecimal characters".into());
    }
    let decoded = hex::decode(text).map_err(|_| "previous SHA-256 value is invalid".to_owned())?;
    decoded
        .try_into()
        .map_err(|_| "previous SHA-256 value is invalid".to_owned())
}

fn apply_service_operation(
    manager: &mut dyn eggup_service::ServiceManager,
    spec: &eggup_service::ServiceSpec,
    verb: &str,
    backend: &str,
) -> Result<(), String> {
    if verb == "status" {
        let snapshot = eggup_service::ServiceManager::inspect(manager, spec)
            .map_err(|error| error.to_string())?;
        return output(&serde_json::json!({
            "schema_version": 1,
            "service_id": spec.id().as_str(),
            "platform": std::env::consts::OS,
            "backend": backend,
            "ownership": format!("{:?}", snapshot.ownership),
            "state": format!("{:?}", snapshot.state),
        }));
    }
    let outcome = match verb {
        "install" => eggup_service::ServiceManager::install(manager, spec),
        "uninstall" => eggup_service::ServiceManager::uninstall(manager, spec),
        "start" => {
            eggup_service::ServiceManager::start(manager, spec, std::time::Duration::from_secs(60))
        }
        "stop" => {
            eggup_service::ServiceManager::stop(manager, spec, std::time::Duration::from_secs(60))
        }
        "restart" => eggup_service::ServiceManager::restart(
            manager,
            spec,
            std::time::Duration::from_secs(60),
        ),
        _ => return Err(usage().into()),
    }
    .map_err(|error| error.to_string())?;
    // Emit the structured report first so the machine-readable record of the
    // attempt survives even on failure, then fail the process. `completed: false`
    // means the manager did not reach the desired end state, so exiting 0 would
    // tell a provisioning script the node was transitioned when it was not —
    // and would contradict our own release path, which treats the same value as
    // a hard failure.
    let completed = outcome.completed();
    let detail = outcome.detail.clone();
    output(
        &serde_json::json!({"schema_version": 1, "service_id": spec.id().as_str(), "platform": std::env::consts::OS, "backend": backend, "operation": verb, "completed": completed}),
    )?;
    if completed {
        return Ok(());
    }
    Err(format!(
        "service {verb} did not complete{}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|position| args.get(position + 1))
        .map(String::as_str)
}

#[cfg(target_os = "macos")]
fn required_path(args: &[String], name: &str) -> Result<std::path::PathBuf, String> {
    let path = option_value(args, name)
        .ok_or_else(|| format!("service lifecycle requires {name} <absolute-path>"))?;
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute() {
        return Err(format!("{name} must be absolute"));
    }
    Ok(path)
}

fn parse_option<T: std::str::FromStr>(
    args: &[String],
    name: &str,
    default: T,
) -> Result<T, String> {
    match args.iter().position(|arg| arg == name) {
        Some(index) => args
            .get(index + 1)
            .ok_or_else(|| format!("{name} requires a value"))?
            .parse()
            .map_err(|_| format!("invalid {name} value")),
        None => Ok(default),
    }
}

fn output(value: &impl Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|_| "output could not be encoded".to_owned())?
    );
    Ok(())
}

fn usage() -> &'static str {
    "usage: eggworkd <run|service-host|config validate|config print|doctor|status|metrics|drain|undrain|executions list|executions show|storage summary|inspect|gc|deployment status|deployment apply|service spec|service status|service install|service uninstall|service start|service stop|service restart|version> --config <file>; deployment apply accepts local --daemon/--helper candidates; service backends require explicit systemd, launchd, or Windows SCM policy; service-host is the Windows service control manager entry and takes --config <absolute-path>"
}

/// The service-host lifecycle tests.
///
/// They live in the binary target on purpose: `cargo test -p eggwork-server
/// --bin eggworkd` exercises them on Windows without compiling the server's
/// own Linux-shaped unit tests, which is what makes this surface reachable
/// from a hosted Windows runner at all.
#[cfg(test)]
mod tests {
    use super::*;
    use eggwork_server::service_host::{ReadyRefused, ServiceExit, ServiceHost, ServiceState};
    use std::time::{Duration, Instant};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn only_the_service_host_verb_enters_the_dispatcher_path() {
        assert_eq!(
            mode_for(&args(&["service-host", "--config", "C:\\node.json"])),
            Mode::ServiceHost
        );
        // Every ordinary verb stays on the CLI path. Foreground `run` keeps
        // its meaning, and nothing else is inferred.
        for command in [
            "run",
            "status",
            "doctor",
            "config",
            "deployment",
            "service",
            "version",
        ] {
            assert_eq!(
                mode_for(&args(&[command, "--config", "node.json"])),
                Mode::Cli,
                "{command} must not enter the service host path"
            );
        }
        assert_eq!(mode_for(&args(&[])), Mode::Cli);
    }

    #[test]
    fn a_running_claim_needs_the_ready_edge() {
        let host = ServiceHost::new();
        assert_eq!(host.state(), ServiceState::StartPending);
        assert!(!host.served());
        assert_eq!(host.ready(), Ok(ServiceState::Running));
        assert!(host.served());
    }

    #[test]
    fn a_stop_during_start_is_latched() {
        let host = ServiceHost::new();
        host.request_stop();
        assert!(host.stop_requested());
        assert_eq!(host.ready(), Err(ReadyRefused::StopLatched));
        // The node never served, so it never claimed to.
        assert!(!host.served());
        assert_eq!(host.state(), ServiceState::StartPending);
    }

    #[test]
    fn repeated_stop_and_shutdown_controls_are_idempotent() {
        let host = ServiceHost::new();
        host.request_stop();
        host.request_stop();
        assert!(host.stop_requested());
        assert_eq!(host.begin_stop(), ServiceState::StopPending);
        assert_eq!(host.begin_stop(), ServiceState::StopPending);
        assert_eq!(host.mark_stopped(), ServiceState::Stopped);
        assert_eq!(host.mark_stopped(), ServiceState::Stopped);
    }

    #[test]
    fn the_control_callback_does_not_block() {
        let host = ServiceHost::new();
        let started = Instant::now();
        host.request_stop();
        host.request_stop();
        // The callback does no work. A control handler that blocked here
        // would stall the service control manager's own thread, and the
        // manager's timeout would turn that into a failed stop.
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "the stop latch took {:?}",
            started.elapsed()
        );
        assert!(host.stop_requested());
    }

    #[test]
    fn stopping_reaches_stopped_only_after_the_convergence_edge() {
        let host = ServiceHost::new();
        host.ready().expect("the node is ready");
        host.begin_stop();
        // Converging is not stopped: the active executions are still being
        // cancelled and waited for.
        assert_eq!(host.state(), ServiceState::StopPending);
        assert_ne!(host.state(), ServiceState::Stopped);
        host.mark_stopped();
        assert_eq!(host.state(), ServiceState::Stopped);
        assert_eq!(host.exit(), ServiceExit::Normal);
    }

    #[test]
    fn a_failed_start_reports_stopped_without_running() {
        let host = ServiceHost::new();
        assert_eq!(host.fail_start(), ServiceExit::StartFailed);
        host.mark_stopped();
        assert_eq!(host.state(), ServiceState::Stopped);
        assert_eq!(host.exit(), ServiceExit::StartFailed);
        assert!(!host.served());
    }
}
