//! The Windows service control manager adapter.
//!
//! This is the only place in the workspace that talks to the Windows service
//! control manager, and it is a *host*, not a *manager*: it registers a
//! control handler, reports truthful lifecycle state, and runs the shared
//! node lifecycle. It never installs, starts, stops, or removes a service —
//! Eggup owns those mutations, through `eggup-service`.
//!
//! The wrapper it uses (`windows-service`) is pinned exactly and is a
//! Windows-target-only dependency. Nothing here needs `unsafe`: the service
//! entry is a plain `extern "system" fn` and the wrapper does the calling
//! convention work.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use windows_service::{
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState as ScmState,
        ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
};

use crate::{
    deployment::SERVICE_ID,
    operations::OperatorConfig,
    service_host::{
        LifecycleEdge, LifecycleError, ServiceExit, ServiceHost, ServiceState, serve_until_stopped,
    },
};

/// How long the manager is told to wait for a pending transition.
///
/// The manager treats this as its timeout for the transition, so it is
/// bounded and it is backed by a checkpoint that advances while the
/// transition is in flight. Without the checkpoint a slow start would look
/// like a hung service, which is exactly the error the previous milestone
/// recorded.
const WAIT_HINT: Duration = Duration::from_secs(30);

/// How often the pending checkpoint advances.
///
/// Well inside the wait hint, so the manager's timer is always reset by a
/// progress report rather than by the transition finishing.
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(10);

/// The service-specific exit code for a node that could not start.
const EXIT_START_FAILED: u32 = 1;

/// The service-specific exit code for a node that served but did not
/// converge.
const EXIT_CONVERGE_FAILED: u32 = 2;

/// The immutable bootstrap handoff.
///
/// The service-entry ABI is `extern "system" fn(u32, *mut *mut u16)`: it
/// carries the service argument table and nothing else, so the one value the
/// service main needs — the configuration path the manager registered — is
/// handed over through this immutable slot before the dispatcher starts.
/// Nothing mutable lives here: no runtime handle, no node, and no lifecycle
/// state.
static SERVICE_CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// The service-host process entry.
///
/// The service control manager launches the registered daemon with the argv
/// `eggworkd service-host --config <absolute-path>`, so the configuration
/// path arrives on the process command line. The mode is never inferred from
/// the parent process, the session, the environment, or a dispatcher error.
pub fn entry() -> std::process::ExitCode {
    // The operator CLI validates TLS server identity locally and the node
    // serves it. No global CryptoProvider is installed by the libraries, so
    // install the process default once; without it every TLS config reports
    // invalid.
    let _ = rustls::crypto::ring::default_provider().install_default();
    match host() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("eggworkd: the service host failed: {error}");
            std::process::ExitCode::from(2)
        }
    }
}

/// Enter the dispatcher and host the service until it stops.
fn host() -> Result<(), String> {
    let config_path = service_config_path()
        .ok_or_else(|| "the service host requires --config <absolute-path>".to_owned())?;
    let _ = SERVICE_CONFIG_PATH.set(config_path);
    // The dispatcher owns this thread until the service stops; the service
    // main runs on its own thread and enters its runtime there.
    service_dispatcher::start(SERVICE_ID, service_main).map_err(|error| error.to_string())
}

/// The configuration path the manager registered.
///
/// It is part of the registered command line, so it is part of this
/// process's own arguments. It is never inferred from the environment or the
/// working directory.
fn service_config_path() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--config" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// The service entry point.
///
/// The manager's service argument table is not used: the configuration path
/// is part of the registered argv, so it was read from the process command
/// line before the dispatcher started.
extern "system" fn service_main(_service_argc: u32, _service_argv: *mut *mut u16) {
    let Some(config_path) = SERVICE_CONFIG_PATH.get() else {
        return;
    };
    if let Err(error) = host_service(config_path) {
        eprintln!("eggworkd: the service host failed: {error}");
    }
}

/// Run the service until the manager stops it.
fn host_service(config_path: &Path) -> Result<(), String> {
    let host = Arc::new(ServiceHost::new());
    let reporter = Arc::new(StatusReporter::new());
    // The control handler is registered before any startup work: the manager
    // may deliver a stop at any point, and a stop that arrives during
    // startup must not be lost.
    let handle = register_control_handler(&reporter, &host).map_err(|error| error.to_string())?;
    reporter.attach(handle);
    reporter.report(ServiceState::StartPending, ServiceExit::Normal);

    // The runtime is built here, on the service thread, rather than on the
    // dispatcher thread: the dispatcher owns its thread until the service
    // exits and must not be parked inside an unrelated runtime.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not build the async runtime: {error}"))?;

    // The manager's wait timer is reset by a checkpoint that advances while a
    // transition is pending.
    let heartbeat = {
        let reporter = Arc::clone(&reporter);
        runtime.spawn(async move {
            let mut ticker = tokio::time::interval(HEARTBEAT_PERIOD);
            // The first tick is immediate; skip it so the heartbeat does not
            // race the first status report.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                reporter.bump();
            }
        })
    };

    let outcome = runtime.block_on(async {
        // The configuration is loaded and validated inside the service, so a
        // bad configuration is reported as a failed start rather than as a
        // process that died before it ever connected to the manager.
        let config = OperatorConfig::load(config_path)
            .map_err(|error| LifecycleError::Start(error.to_string()))?;
        config
            .validate()
            .map_err(|error| LifecycleError::Start(error.to_string()))?;
        serve(&config, &host, &reporter).await
    });
    heartbeat.abort();

    match outcome {
        Ok(()) => Ok(()),
        Err(error) => {
            let exit = match error {
                LifecycleError::Start(_) => host.fail_start(),
                LifecycleError::Converge(_) => host.fail_convergence(),
            };
            host.mark_stopped();
            reporter.report(ServiceState::Stopped, exit);
            Err(error.to_string())
        }
    }
}

/// Run the shared node lifecycle, reporting the edges to the manager.
async fn serve(
    config: &OperatorConfig,
    host: &Arc<ServiceHost>,
    reporter: &Arc<StatusReporter>,
) -> Result<(), LifecycleError> {
    let stop_requested = Arc::clone(host);
    let mut stopped = host.stop_receiver();
    let edge_host = Arc::clone(host);
    let edge_reporter = Arc::clone(reporter);
    serve_until_stopped(
        config,
        || stop_requested.stop_requested(),
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
        move |edge| match edge {
            LifecycleEdge::Ready(_) => {
                // The node is ready. `RUNNING` is claimed only if no stop was
                // latched while it was starting; a stop that arrived during
                // startup means the node never served.
                if edge_host.ready().is_ok() {
                    edge_reporter.report(ServiceState::Running, ServiceExit::Normal);
                }
            }
            LifecycleEdge::Converging => {
                edge_host.begin_stop();
                edge_reporter.report(ServiceState::StopPending, ServiceExit::Normal);
            }
            LifecycleEdge::Converged => {
                edge_host.mark_stopped();
                edge_reporter.report(ServiceState::Stopped, edge_host.exit());
            }
        },
    )
    .await
}

/// Register the control handler.
///
/// The handler is a signal edge and nothing more: it latches a stop request
/// and returns. It performs no network, database, sleep, wait, runner
/// cleanup, or service-manager operation. The only other thing it does is
/// answer an interrogate, which the manager's contract requires the service
/// to do by reporting its current status.
fn register_control_handler(
    reporter: &Arc<StatusReporter>,
    host: &Arc<ServiceHost>,
) -> Result<ServiceStatusHandle, windows_service::Error> {
    let reporter = Arc::clone(reporter);
    let host = Arc::clone(host);
    service_control_handler::register(SERVICE_ID, move |control| match control {
        ServiceControl::Interrogate => {
            reporter.report_current(host.as_ref());
            ServiceControlHandlerResult::NoError
        }
        // Stop and shutdown are the same request: the node stops admitting
        // work, converges, and reports `STOPPED`.
        ServiceControl::Stop | ServiceControl::Shutdown => {
            host.request_stop();
            ServiceControlHandlerResult::NoError
        }
        // Nothing else is implemented, so nothing else is advertised as
        // accepted.
        _ => ServiceControlHandlerResult::NotImplemented,
    })
}

/// What the reporter last told the manager.
#[derive(Debug, Clone, Copy)]
struct ReportedState {
    state: ServiceState,
    exit: ServiceExit,
    checkpoint: u32,
}

/// Reports lifecycle state to the service control manager.
///
/// Every write is serialised, and the pending checkpoint only advances while
/// a transition is actually pending, so the manager never sees a stale
/// checkpoint on a settled state.
#[derive(Debug)]
struct StatusReporter {
    handle: OnceLock<ServiceStatusHandle>,
    reported: Mutex<ReportedState>,
}

impl StatusReporter {
    fn new() -> Self {
        Self {
            handle: OnceLock::new(),
            reported: Mutex::new(ReportedState {
                state: ServiceState::StartPending,
                exit: ServiceExit::Normal,
                checkpoint: 0,
            }),
        }
    }

    fn attach(&self, handle: ServiceStatusHandle) {
        let _ = self.handle.set(handle);
    }

    /// Report a state transition.
    ///
    /// A transition restarts the manager's wait timer, so the checkpoint
    /// returns to zero.
    fn report(&self, state: ServiceState, exit: ServiceExit) {
        let Some(handle) = self.handle.get() else {
            return;
        };
        let status = {
            let mut reported = self
                .reported
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            reported.state = state;
            reported.exit = exit;
            reported.checkpoint = 0;
            build_status(state, exit, reported.checkpoint)
        };
        let _ = handle.set_service_status(status);
    }

    /// Advance the pending checkpoint.
    ///
    /// This is the heartbeat that keeps a long transition from looking like a
    /// hung service. It does nothing once the transition is over.
    fn bump(&self) {
        let Some(handle) = self.handle.get() else {
            return;
        };
        let status = {
            let mut reported = self
                .reported
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !reported.state.is_pending() {
                return;
            }
            reported.checkpoint = reported.checkpoint.saturating_add(1);
            build_status(reported.state, reported.exit, reported.checkpoint)
        };
        let _ = handle.set_service_status(status);
    }

    /// Report the host's current state, for an interrogate.
    fn report_current(&self, host: &ServiceHost) {
        let Some(handle) = self.handle.get() else {
            return;
        };
        let state = host.state();
        let exit = host.exit();
        let status = {
            let mut reported = self
                .reported
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            reported.state = state;
            reported.exit = exit;
            build_status(state, exit, reported.checkpoint)
        };
        let _ = handle.set_service_status(status);
    }
}

/// Build the status the manager is told about.
fn build_status(state: ServiceState, exit: ServiceExit, checkpoint: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: scm_state(state),
        controls_accepted: match state {
            // The node accepts a stop while it serves. Nothing else is
            // implemented, so nothing else is advertised: the manager only
            // delivers the controls a service says it accepts.
            ServiceState::Running => ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            _ => ServiceControlAccept::empty(),
        },
        exit_code: scm_exit(exit),
        checkpoint,
        wait_hint: if state.is_pending() {
            WAIT_HINT
        } else {
            Duration::ZERO
        },
        process_id: None,
    }
}

/// The manager's state for one of ours.
fn scm_state(state: ServiceState) -> ScmState {
    match state {
        ServiceState::StartPending => ScmState::StartPending,
        ServiceState::Running => ScmState::Running,
        ServiceState::StopPending => ScmState::StopPending,
        ServiceState::Stopped => ScmState::Stopped,
    }
}

/// The exit code the manager is told about.
fn scm_exit(exit: ServiceExit) -> ServiceExitCode {
    match exit {
        ServiceExit::Normal => ServiceExitCode::NO_ERROR,
        ServiceExit::StartFailed => ServiceExitCode::ServiceSpecific(EXIT_START_FAILED),
        ServiceExit::ConvergeFailed => ServiceExitCode::ServiceSpecific(EXIT_CONVERGE_FAILED),
    }
}
