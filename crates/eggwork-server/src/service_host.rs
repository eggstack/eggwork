//! The service host: one node lifecycle, and the states a hosted node reports.
//!
//! This module is deliberately free of Win32 calls. The SCM wrapper drives it
//! and reports the states it produces; the state machine itself decides
//! *which* state comes next and *whether* a transition is allowed. That
//! separation is what makes the transitions testable on any host, and it is
//! what keeps the control callback free of work: the callback latches a stop
//! request and returns, and the service thread does the reporting.
//!
//! The states are the ones the Windows service control manager understands:
//!
//! ```text
//! START_PENDING
//!       |
//!       | configuration valid + the node is ready
//!       v
//! RUNNING
//!       |
//!       | STOP or SHUTDOWN received
//!       v
//! STOP_PENDING
//!       |
//!       | the shared shutdown and wait converged
//!       v
//! STOPPED
//! ```

use std::{
    future::Future,
    net::SocketAddr,
    sync::atomic::{AtomicU8, Ordering},
};

use tokio::sync::watch;

use crate::{NodeServer, operations::OperatorConfig};

#[cfg(windows)]
mod windows;

/// The state a service-hosted node reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// The node is starting.
    ///
    /// Reported before any potentially blocking startup work, so the service
    /// control manager does not conclude that the service hung.
    StartPending,
    /// The node is serving.
    Running,
    /// The node is converging: admission has stopped, active executions are
    /// being cancelled, and the listener is closing.
    StopPending,
    /// The node has stopped.
    Stopped,
}

impl ServiceState {
    fn as_u8(self) -> u8 {
        match self {
            ServiceState::StartPending => 0,
            ServiceState::Running => 1,
            ServiceState::StopPending => 2,
            ServiceState::Stopped => 3,
        }
    }

    fn from_u8(value: u8) -> ServiceState {
        match value & 0b11 {
            0 => ServiceState::StartPending,
            1 => ServiceState::Running,
            2 => ServiceState::StopPending,
            _ => ServiceState::Stopped,
        }
    }

    /// Whether the state is a transition the manager is waiting on.
    ///
    /// A pending state is the only one that may carry a non-zero checkpoint:
    /// the manager resets its wait timer when the checkpoint advances, and a
    /// checkpoint on a settled state is a lie about progress that is not
    /// happening.
    pub fn is_pending(self) -> bool {
        matches!(self, ServiceState::StartPending | ServiceState::StopPending)
    }
}

/// How the service stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceExit {
    /// A normal stop: the service was asked to stop and converged.
    Normal,
    /// The node could not start, so the service never served.
    StartFailed,
    /// The node served but did not converge when it was asked to stop.
    ConvergeFailed,
}

impl ServiceExit {
    /// The recorded value.
    ///
    /// Zero is *unset*: no disposition has been recorded yet. Every other
    /// value is a disposition, and the first one recorded wins.
    fn as_u8(self) -> u8 {
        match self {
            ServiceExit::Normal => 1,
            ServiceExit::StartFailed => 2,
            ServiceExit::ConvergeFailed => 3,
        }
    }

    fn from_u8(value: u8) -> ServiceExit {
        match value {
            2 => ServiceExit::StartFailed,
            3 => ServiceExit::ConvergeFailed,
            _ => ServiceExit::Normal,
        }
    }
}

/// Why the running claim was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyRefused {
    /// A stop request arrived while the node was still starting.
    ///
    /// The latch is what carries the intent across the startup edge: a
    /// control that arrives during `START_PENDING` must not be lost, and it
    /// must not produce a transient `RUNNING` claim either.
    StopLatched,
    /// The host was no longer starting, so there was nothing to claim.
    NotStarting,
}

/// The service host's state machine.
///
/// The state and the stop latch live in **one** atomic byte so that the
/// decision to claim `RUNNING` and the arrival of a stop request cannot
/// interleave. A separate notification channel carries the *wake-up*; it is
/// allowed to be late or duplicated, because the byte is the truth.
#[derive(Debug)]
pub struct ServiceHost {
    /// The low two bits are the state; the third bit is the stop latch.
    raw: AtomicU8,
    /// How the service ended. The first disposition recorded wins.
    exit: AtomicU8,
    /// The notification that a stop was requested.
    notify: watch::Sender<bool>,
}

const STOP_REQUESTED: u8 = 0b100;

/// The bit that records that the node ever claimed to be serving.
///
/// The claim is a fact about what the service *told* the manager, so it is
/// remembered separately from the current state: a host that failed to start
/// ends in `STOPPED`, and the question "did it ever claim to be running?"
/// still has to have an answer.
const SERVED: u8 = 0b1000;

impl Default for ServiceHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceHost {
    /// A host in `START_PENDING`.
    pub fn new() -> Self {
        let (notify, _receiver) = watch::channel(false);
        Self {
            raw: AtomicU8::new(ServiceState::StartPending.as_u8()),
            exit: AtomicU8::new(0),
            notify,
        }
    }

    /// The state to report.
    pub fn state(&self) -> ServiceState {
        ServiceState::from_u8(self.raw.load(Ordering::SeqCst))
    }

    /// How the service ended.
    pub fn exit(&self) -> ServiceExit {
        ServiceExit::from_u8(self.exit.load(Ordering::SeqCst))
    }

    /// Whether a stop has been requested.
    pub fn stop_requested(&self) -> bool {
        self.raw.load(Ordering::SeqCst) & STOP_REQUESTED != 0
    }

    /// Whether the node ever claimed to be serving.
    ///
    /// A node that failed to start never claims to be running, and a node
    /// whose stop arrived during startup never claims it either: the claim
    /// is what the manager was told, and it is remembered so a later
    /// disposition cannot paper over it.
    pub fn served(&self) -> bool {
        self.raw.load(Ordering::SeqCst) & SERVED != 0
    }

    /// Latch a stop request.
    ///
    /// This is the only thing the control callback does. It is idempotent:
    /// repeated `STOP` and `SHUTDOWN` controls are the same request, and it
    /// performs no network, database, sleep, wait, runner cleanup, or
    /// service-manager operation.
    pub fn request_stop(&self) {
        let mut current = self.raw.load(Ordering::SeqCst);
        loop {
            let next = current | STOP_REQUESTED;
            match self
                .raw
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        // The latch is set before the notification is sent, so a waiter that
        // wakes sees the request.
        let _ = self.notify.send(true);
    }

    /// A receiver that observes stop requests.
    pub fn stop_receiver(&self) -> watch::Receiver<bool> {
        self.notify.subscribe()
    }

    /// The node is serving.
    ///
    /// Returns `Err(ReadyRefused::StopLatched)` when a stop was requested
    /// while the node was starting. The node never ran in that case, so the
    /// caller must converge and report `STOPPED` — it must never claim
    /// `RUNNING` and take it back.
    pub fn ready(&self) -> Result<ServiceState, ReadyRefused> {
        match self.raw.compare_exchange(
            ServiceState::StartPending.as_u8(),
            ServiceState::Running.as_u8() | SERVED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => Ok(ServiceState::Running),
            Err(observed) => {
                if observed & STOP_REQUESTED != 0 {
                    Err(ReadyRefused::StopLatched)
                } else if observed & 0b11 == ServiceState::Running.as_u8() {
                    // Already running: claiming it again is not an error.
                    Ok(ServiceState::Running)
                } else {
                    Err(ReadyRefused::NotStarting)
                }
            }
        }
    }

    /// The node is converging.
    ///
    /// Idempotent: a repeated request leaves the state where it is.
    pub fn begin_stop(&self) -> ServiceState {
        self.set_state(ServiceState::StopPending)
    }

    /// The node has stopped.
    ///
    /// Idempotent, and it never moves the state backwards. The exit
    /// disposition is recorded here if nothing has recorded one yet: a node
    /// that stopped normally is not retroactively reported as a failed start,
    /// and a node that failed to start keeps its failure.
    pub fn mark_stopped(&self) -> ServiceState {
        self.record_exit(ServiceExit::Normal);
        self.set_state(ServiceState::Stopped)
    }

    /// The node could not start.
    ///
    /// The first disposition recorded wins, so a node that already stopped
    /// normally is not retroactively reported as a failed start.
    pub fn fail_start(&self) -> ServiceExit {
        self.record_exit(ServiceExit::StartFailed)
    }

    /// The node served but did not converge.
    pub fn fail_convergence(&self) -> ServiceExit {
        self.record_exit(ServiceExit::ConvergeFailed)
    }

    fn record_exit(&self, exit: ServiceExit) -> ServiceExit {
        let _ = self
            .exit
            .compare_exchange(0, exit.as_u8(), Ordering::SeqCst, Ordering::SeqCst);
        self.exit()
    }

    fn set_state(&self, state: ServiceState) -> ServiceState {
        let mut current = self.raw.load(Ordering::SeqCst);
        loop {
            let next = (current & !0b11) | state.as_u8();
            match self
                .raw
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return ServiceState::from_u8(next),
                Err(observed) => current = observed,
            }
        }
    }
}

/// What the shared node lifecycle reports to its caller.
///
/// The edges are the only thing the two callers differ on: the foreground
/// path prints the ready line, and the service-hosted path reports the
/// states to the service control manager. The lifecycle itself is the same
/// function either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEdge {
    /// The node is ready to serve: the listener is bound and the control
    /// plane is accepting connections. The address is the one the node
    /// actually bound, which is not always the configured one.
    Ready(SocketAddr),
    /// The node has stopped admitting work and is converging: the active
    /// executions are being cancelled and the listener is closing.
    Converging,
    /// The node converged: the listener closed and every active execution
    /// finished.
    Converged,
}

/// Why the shared node lifecycle ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    /// The node could not start.
    Start(String),
    /// The node started but did not converge.
    Converge(String),
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::Start(detail) => write!(formatter, "the node did not start: {detail}"),
            LifecycleError::Converge(detail) => {
                write!(formatter, "the node did not converge: {detail}")
            }
        }
    }
}

/// Start the node and return its handle.
///
/// This is the first step of the shared lifecycle, and the only place a node
/// is started. The service-hosted path and the foreground path both come
/// through here, so there is exactly one server and exactly one startup.
pub async fn start_node(config: &OperatorConfig) -> Result<NodeServer, LifecycleError> {
    crate::operations::start_server(config)
        .await
        .map_err(|error| LifecycleError::Start(error.to_string()))
}

/// Converge the node and wait for it to finish.
///
/// The convergence is the in-memory drain: admission stops, the active
/// executions are cancelled, the listener is closed, and the wait observes
/// the executions finishing. It does **not** create a persistent drain
/// marker — that marker is an explicit operator and update mechanism, and a
/// routine stop must not leave the node permanently drained.
pub async fn converge_node(server: NodeServer) -> Result<(), LifecycleError> {
    server.shutdown().await;
    server
        .wait()
        .await
        .map_err(|error| LifecycleError::Converge(error.to_string()))
}

/// The node lifecycle, shared by the foreground and the service-hosted paths.
///
/// Both paths run this exact function, so they converge the same way: stop
/// admission, cancel and converge the active executions, shut the listener
/// down, and **wait** for the listener and the executions to finish before
/// the process exits. Foreground `run` used to call `shutdown()` without
/// `wait()`, so a foreground stop could return while an execution was still
/// converging.
///
/// `stop_requested` is consulted once the node is ready. A stop that arrived
/// while the node was starting means the node never served, so it is
/// converged without a readiness edge: claiming that it served would be a
/// false claim about a node the operator had already stopped.
///
/// `await_stop` is awaited after the readiness edge, and must resolve when a
/// stop is requested.
///
/// `observe` receives the lifecycle edges. The service-hosted path reports
/// them to the service control manager; the foreground path ignores them.
pub async fn serve_until_stopped<S, A, O>(
    config: &OperatorConfig,
    stop_requested: S,
    await_stop: A,
    mut observe: O,
) -> Result<(), LifecycleError>
where
    S: Fn() -> bool,
    A: Future<Output = ()>,
    O: FnMut(LifecycleEdge),
{
    let server = start_node(config).await?;
    if !stop_requested() {
        observe(LifecycleEdge::Ready(server.local_addr()));
        await_stop.await;
    }
    observe(LifecycleEdge::Converging);
    converge_node(server).await?;
    observe(LifecycleEdge::Converged);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_new_host_is_start_pending() {
        let host = ServiceHost::new();
        assert_eq!(host.state(), ServiceState::StartPending);
        assert!(!host.stop_requested());
        assert_eq!(host.exit(), ServiceExit::Normal);
    }

    #[test]
    fn running_is_claimed_only_after_the_ready_edge() {
        let host = ServiceHost::new();
        assert_eq!(host.state(), ServiceState::StartPending);
        assert!(!host.served());
        assert_eq!(host.ready(), Ok(ServiceState::Running));
        assert_eq!(host.state(), ServiceState::Running);
        assert!(host.served());
    }

    #[test]
    fn a_failed_start_never_claimed_to_be_running() {
        let host = ServiceHost::new();
        assert_eq!(host.fail_start(), ServiceExit::StartFailed);
        host.mark_stopped();
        assert_eq!(host.state(), ServiceState::Stopped);
        assert!(!host.served());
    }

    #[test]
    fn a_stop_during_start_is_latched_and_refuses_the_running_claim() {
        let host = ServiceHost::new();
        host.request_stop();
        assert!(host.stop_requested());
        assert_eq!(host.ready(), Err(ReadyRefused::StopLatched));
        assert_eq!(host.state(), ServiceState::StartPending);
        assert!(!host.served());
    }

    #[test]
    fn repeated_stops_are_idempotent() {
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
    fn the_stop_callback_returns_without_waiting() {
        let host = ServiceHost::new();
        let started = Instant::now();
        host.request_stop();
        host.request_stop();
        // The callback does no work; a control handler that blocked here
        // would stall the service control manager's own thread.
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "the stop latch took {:?}",
            started.elapsed()
        );
        assert!(host.stop_requested());
    }

    #[test]
    fn a_stopped_host_is_only_stopped_after_the_stop_edge() {
        let host = ServiceHost::new();
        host.ready().expect("the node is ready");
        host.begin_stop();
        assert_eq!(host.state(), ServiceState::StopPending);
        // Converging is not stopped: the service is still finishing its
        // executions.
        assert_ne!(host.state(), ServiceState::Stopped);
        host.mark_stopped();
        assert_eq!(host.state(), ServiceState::Stopped);
    }

    #[test]
    fn the_first_disposition_wins() {
        let host = ServiceHost::new();
        assert_eq!(host.fail_start(), ServiceExit::StartFailed);
        // A node that already reported a failed start is not retroactively
        // reported as a normal stop.
        host.mark_stopped();
        assert_eq!(host.exit(), ServiceExit::StartFailed);

        let other = ServiceHost::new();
        other.mark_stopped();
        assert_eq!(other.fail_start(), ServiceExit::Normal);
    }

    #[test]
    fn a_stopped_host_wakes_its_waiter() {
        let host = ServiceHost::new();
        let receiver = host.stop_receiver();
        assert!(!*receiver.borrow());
        host.request_stop();
        assert!(*receiver.borrow());
    }

    #[test]
    fn pending_states_are_the_only_ones_that_carry_a_checkpoint() {
        assert!(ServiceState::StartPending.is_pending());
        assert!(ServiceState::StopPending.is_pending());
        assert!(!ServiceState::Running.is_pending());
        assert!(!ServiceState::Stopped.is_pending());
    }
}
