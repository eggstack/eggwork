use eggwork_server::deployment;
use eggwork_server::operations::{
    OperatorConfig, collect_garbage, doctor, execution_page, execution_show, inspect_artifact,
    inspect_blob, inspect_workspace, is_persistently_draining, metrics_snapshot,
    set_persistent_drain, start_server, storage_summary,
};
use serde::Serialize;
use std::{env, path::PathBuf, process::ExitCode};

#[tokio::main]
async fn main() -> ExitCode {
    // The operator CLI validates TLS server identity locally (config
    // validate/doctor/status/deployment/service) and serves it (run). No
    // global CryptoProvider is installed by the libraries, so install the
    // process default once; without it every TLS config reports invalid.
    let _ = rustls::crypto::ring::default_provider().install_default();
    match run(env::args().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("eggworkd: {error}");
            ExitCode::from(2)
        }
    }
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
    let config = OperatorConfig::load(&config_path)
        .map_err(|_| "configuration is invalid or unreadable".to_owned())?;
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
                    config
                        .redacted_json()
                        .map_err(|_| "configuration could not be rendered".to_owned())?
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
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
            let page = execution_page(&config, 1, 0)
                .await
                .map_err(|_| "execution state is unavailable".to_owned())?;
            let metrics = metrics_snapshot(&config)
                .map_err(|_| "operator metrics are unavailable".to_owned())?;
            output(
                &serde_json::json!({"schema_version":1,"node_id":config.node_id,"draining":is_persistently_draining(&config.drain_path()),"active_executions":page.active_executions,"max_active_executions":config.max_active_executions,"execution_records":page.total,"execution_states":page.states,"stdout_bytes":page.stdout_bytes,"stderr_bytes":page.stderr_bytes,"cleanup_failures":page.cleanup_failures,"metrics":metrics,"doctor":doctor(&config).await}),
            )
        }
        "drain" | "undrain" => {
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
            let draining = command == "drain";
            set_persistent_drain(&config.drain_path(), draining)
                .map_err(|_| "drain state could not be updated".to_owned())?;
            output(&serde_json::json!({"draining":draining,"persistent":true}))
        }
        "executions" => {
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
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
        "storage" => {
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
            match args.get(index).map(String::as_str) {
                Some("summary") => output(
                    &storage_summary(&config)
                        .map_err(|_| "storage summary is unavailable".to_owned())?,
                ),
                _ => Err(usage().into()),
            }
        }
        "inspect" => {
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
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
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
            let dry_run = !args[index..].iter().any(|arg| arg == "--apply");
            let limit = parse_option(&args[index..], "--limit", 128usize)?.clamp(1, 1024);
            output(
                &collect_garbage(&config, dry_run, limit)
                    .await
                    .map_err(|_| "bounded garbage collection failed".to_owned())?,
            )
        }
        "run" => {
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
            let server = start_server(&config)
                .await
                .map_err(|_| "node failed to start; check doctor and service logs".to_owned())?;
            println!(
                "{}",
                serde_json::json!({"ready":true,"node_id":config.node_id,"local_addr":server.local_addr()})
            );
            tokio::signal::ctrl_c()
                .await
                .map_err(|_| "could not wait for shutdown signal".to_owned())?;
            server.shutdown().await;
            Ok(())
        }
        "deployment" => {
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
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
                _ => Err(usage().into()),
            }
        }
        "service" => {
            config
                .validate()
                .map_err(|_| "configuration is invalid".to_owned())?;
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
            if verb == "status" {
                let facts = deployment::platform_support().map_err(|error| error.to_string())?;
                return output(
                    &serde_json::json!({"schema_version": 1, "service_id": spec.id().as_str(), "platform": {"os": facts.os, "systemd_available": facts.systemd_available, "launchd_available": facts.launchd_available, "crontab_available": facts.crontab_available}}),
                );
            }
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
    if std::env::consts::OS != "linux" {
        return Err(format!(
            "service {verb} is not qualified on this platform; refusing without hosted evidence"
        ));
    }
    let unit_path = std::path::PathBuf::from(
        args.iter()
            .position(|arg| arg == "--unit-path")
            .and_then(|position| args.get(position + 1))
            .ok_or_else(|| "service lifecycle requires --unit-path <absolute-path>".to_owned())?,
    );
    let scope_text = args
        .iter()
        .position(|arg| arg == "--scope")
        .and_then(|position| args.get(position + 1))
        .map(String::as_str)
        .unwrap_or("system");
    let scope = deployment::parse_systemd_scope(scope_text).map_err(|error| error.to_string())?;
    let unit_name = unit_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "unit path must name a *.service file".to_owned())?
        .to_owned();
    let enable = args.iter().any(|arg| arg == "--enable");
    let reload = !args.iter().any(|arg| arg == "--no-reload");
    let mut manager = deployment::systemd_manager(
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
    let outcome = match verb {
        "install" => eggup_service::ServiceManager::install(&mut manager, spec),
        "uninstall" => eggup_service::ServiceManager::uninstall(&mut manager, spec),
        "start" => eggup_service::ServiceManager::start(
            &mut manager,
            spec,
            std::time::Duration::from_secs(60),
        ),
        "stop" => eggup_service::ServiceManager::stop(
            &mut manager,
            spec,
            std::time::Duration::from_secs(60),
        ),
        "restart" => eggup_service::ServiceManager::restart(
            &mut manager,
            spec,
            std::time::Duration::from_secs(60),
        ),
        _ => return Err(usage().into()),
    }
    .map_err(|error| error.to_string())?;
    output(
        &serde_json::json!({"schema_version": 1, "service_id": spec.id().as_str(), "operation": verb, "completed": outcome.completed()}),
    )
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
    "usage: eggworkd <run|config validate|config print|doctor|status|drain|undrain|executions list|executions show|storage summary|inspect|gc|deployment status|service spec|service status|service install|service uninstall|service start|service stop|service restart|version> --config <file>"
}
