//! The platform process-tree owner behind [`LocalProcessRunner`](crate::LocalProcessRunner).
//!
//! There is exactly one runner lifecycle in `lib.rs`; this module is the only
//! place where "how do I create, own, wait for, and terminate a process tree"
//! differs between platforms. Everything else — request validation, command
//! construction, pipe readers, bounded capture, timeout/cancellation selection,
//! terminal conversion, and provenance — is shared, so a second runner can never
//! grow here.
//!
//! # Why the tree owner is not `tokio::process::Child`
//!
//! A bare child owns *itself*. Eggwork's invariant is stronger:
//!
//! > process-tree cleanup completes before the execution owner releases its
//! > lifetime permit.
//!
//! On Unix the leader is spawned into its own process group, so signalling the
//! group reaches every descendant the target creates. On Windows there is no
//! process group and `GenerateConsoleCtrlEvent` is not a tree-wide, catchable
//! signal for an arbitrary argv target, so the tree owner is a Job Object.
//!
//! The Job Object must exist *before* the first instruction of the target runs.
//! Spawning normally and calling `AssignProcessToJobObject` afterwards leaves a
//! window in which the target has already run and already created descendants
//! that were never owned. [`process_wrap::tokio::JobObject`] closes that window
//! by adding `CREATE_SUSPENDED` to the creation flags, assigning the still
//! suspended process, and only then resuming it. Spawn failure, assignment
//! failure, and resume failure all terminate the created process before
//! returning, so no target is left running when setup fails.
//!
//! # What Windows convergence does and does not claim
//!
//! Windows has no `SIGTERM`/`SIGKILL` escalation to emulate, and the invariant is
//! deterministic tree convergence rather than signal fidelity. Termination is
//! therefore `TerminateJobObject` on the owned Job Object, which the kernel
//! applies to every member of the job.
//!
//! Eggwork does **not** claim to observe each descendant's final exit status: no
//! safe-Rust surface in the reviewed dependency exposes a job-membership query,
//! and adding one would mean Eggwork-owned `unsafe` Win32 FFI, which this
//! repository denies at the workspace level. Instead convergence is established
//! by three facts that the shared lifecycle relies on:
//!
//! 1. the Job Object stays owned for the whole execution, so no descendant can
//!    ever escape the tree by outliving a leader;
//! 2. after `TerminateJobObject` returns, the kernel has suspended and marked
//!    every member terminating, so no member can execute further user code; and
//! 3. the shared lifecycle joins the pipe readers after termination, and a pipe
//!    only reaches EOF once every process holding it has exited — the same
//!    evidence the Unix path has always used.
//!
//! Closing the Job Object handle is the final backstop: the job is created with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so the kernel terminates any member that
//! somehow survived, and that close happens when the tree owner is dropped —
//! before `run` returns, and therefore before the caller's execution permit is
//! released. Kill-on-drop is the backstop, never a substitute for the explicit
//! normal-path termination above.

use std::process::ExitStatus;
use std::time::Duration;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};

/// What converging an owned process tree produced.
///
/// The three fields map one-to-one onto the runner's existing
/// `CleanupDiagnostics`, so no new public diagnostic shape is introduced and no
/// wire schema changes.
pub(crate) struct TreeConvergence {
    /// The leader's exit status, when the platform reaped it.
    pub(crate) status: Option<ExitStatus>,
    /// Tree termination itself failed, so descendants may have survived.
    pub(crate) signal_error: Option<String>,
    /// The leader did not get reaped within the granted bound.
    pub(crate) wait_error: Option<String>,
}

impl TreeConvergence {
    fn converged(status: Option<ExitStatus>) -> Self {
        Self {
            status,
            signal_error: None,
            wait_error: None,
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use crate::{RunnerError, TERMINATION_GRACE};
    use tokio::{process::Child, time::timeout};

    /// Unix tree owner: the leader plus the process group it was spawned into.
    ///
    /// `group` is captured at spawn time on purpose. `tokio::process::Child::id`
    /// returns `None` once the child has been reaped, and the group identifier is
    /// still needed after that point to reap descendants the leader left behind.
    pub(crate) struct ProcessTree {
        child: Child,
        group: Option<u32>,
    }

    impl ProcessTree {
        /// Spawn the already-sanitised command as the leader of a new process
        /// group. Every descendant the target creates inherits that group, which
        /// is what makes `killpg` a tree operation.
        pub(crate) fn spawn_platform(mut command: Command) -> Result<Self, RunnerError> {
            command.process_group(0);
            let child = command
                .spawn()
                .map_err(|error| RunnerError::Spawn(error.to_string()))?;
            let group = child.id();
            Ok(Self { child, group })
        }

        /// The native child, for the Linux sandbox helper handshake, which reads
        /// the helper's status channel before the target exists.
        #[cfg(target_os = "linux")]
        pub(crate) fn child_mut(&mut self) -> &mut Child {
            &mut self.child
        }

        pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
            self.child.stdin.take()
        }

        pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
            self.child.stdout.take()
        }

        pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
            self.child.stderr.take()
        }

        pub(crate) async fn wait(&mut self) -> std::io::Result<ExitStatus> {
            self.child.wait().await
        }

        /// Kill the leader alone, without touching the rest of the group. Used
        /// by the Linux sandbox handshake's refusal paths, where no target
        /// process exists yet and the helper is the only process involved.
        pub(crate) fn start_kill(&mut self) -> std::io::Result<()> {
            self.child.start_kill()
        }

        /// The leader exited on its own. Background descendants it left behind
        /// still hold the output pipes, so the group is reaped before returning
        /// without changing the leader's already-known exit classification.
        pub(crate) async fn converge_after_leader_exit(&mut self) -> TreeConvergence {
            match terminate_group(self.group) {
                Ok(true) => {
                    tokio::time::sleep(TERMINATION_GRACE).await;
                    match kill_group(self.group) {
                        Ok(()) => TreeConvergence::converged(None),
                        Err(error) => TreeConvergence {
                            status: None,
                            signal_error: Some(error),
                            wait_error: None,
                        },
                    }
                }
                Ok(false) => TreeConvergence::converged(None),
                Err(error) => TreeConvergence {
                    status: None,
                    signal_error: Some(error),
                    wait_error: None,
                },
            }
        }

        /// Timeout, cancellation, or output-limit termination: ask the group to
        /// stop, let it have `grace`, then force it and reap the leader.
        ///
        /// `grace` is honoured in both places it applies: the wait between
        /// `SIGTERM` and `SIGKILL`, and the bounded reap afterwards. It used to
        /// sleep the module constant here while still using the parameter for the
        /// reap, so a caller passing anything other than `TERMINATION_GRACE` got a
        /// silent no-op for half its request.
        pub(crate) async fn terminate_and_reap(&mut self, grace: Duration) -> TreeConvergence {
            let mut signal_error = match terminate_group(self.group) {
                Ok(true) => {
                    tokio::time::sleep(grace).await;
                    None
                }
                Ok(false) => None,
                Err(error) => Some(error),
            };
            if let Err(error) = kill_group(self.group) {
                signal_error.get_or_insert(error);
            }
            let (status, wait_error) = match timeout(grace, self.child.wait()).await {
                Ok(Ok(status)) => (Some(status), None),
                Ok(Err(error)) => (None, Some(error.to_string())),
                Err(_) => (None, Some("child did not exit after SIGKILL".into())),
            };
            TreeConvergence {
                status,
                signal_error,
                wait_error,
            }
        }
    }

    fn terminate_group(pid: Option<u32>) -> Result<bool, String> {
        let pid = pid.ok_or_else(|| "child pid is unavailable".to_owned())?;
        match nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        ) {
            Ok(()) => Ok(true),
            Err(nix::errno::Errno::ESRCH) => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    fn kill_group(pid: Option<u32>) -> Result<(), String> {
        let pid = pid.ok_or_else(|| "child pid is unavailable".to_owned())?;
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        )
        .or_else(|error| {
            if error == nix::errno::Errno::ESRCH {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::RunnerError;
    use process_wrap::tokio::{ChildWrapper, CommandWrap, JobObject, KillOnDrop};

    /// Windows tree owner: a Job Object that owns the leader from before its
    /// first instruction, plus every descendant it will ever create.
    pub(crate) struct ProcessTree {
        child: Box<dyn ChildWrapper>,
    }

    impl ProcessTree {
        pub(crate) fn spawn_platform(command: Command) -> Result<Self, RunnerError> {
            let mut command = CommandWrap::from(command);
            // `KillOnDrop` is registered first so the Job Object is created with
            // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: dropping this owner must not
            // orphan a descendant. It also sets Tokio's own kill-on-drop on the
            // leader, which stays in effect underneath the job wrapper.
            command.wrap(KillOnDrop).wrap(JobObject);
            // The child is created suspended, assigned to the job, and resumed
            // inside `spawn`; a failure on any of those steps terminates the
            // process before returning an error.
            let child = command
                .spawn()
                .map_err(|error| RunnerError::Spawn(error.to_string()))?;
            Ok(Self { child })
        }

        pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
            self.child.stdin().take()
        }

        pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
            self.child.stdout().take()
        }

        pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
            self.child.stderr().take()
        }

        pub(crate) async fn wait(&mut self) -> std::io::Result<ExitStatus> {
            self.child.wait().await
        }

        /// The leader exited on its own, so any descendant it left behind is
        /// still owned by the Job Object and is terminated here.
        ///
        /// No exit status is reported for descendants: the job-membership query
        /// that would be required is not available without Eggwork-owned `unsafe`
        /// FFI. The reader EOF barrier the shared lifecycle joins after this call
        /// is the convergence evidence, and the job handle closes under
        /// kill-on-job-close before the runner returns.
        pub(crate) async fn converge_after_leader_exit(&mut self) -> TreeConvergence {
            match self.child.start_kill() {
                Ok(()) => TreeConvergence::converged(None),
                Err(error) => TreeConvergence {
                    status: None,
                    signal_error: Some(error.to_string()),
                    wait_error: None,
                },
            }
        }

        /// Timeout, cancellation, or output-limit termination. `TerminateJobObject`
        /// covers the leader and every descendant in one call, so there is no
        /// Unix-style escalation window and nothing to wait for beyond the call
        /// returning.
        ///
        /// The leader's exit status is deliberately not awaited: a job
        /// termination exit code describes Eggwork's kill, not the target's own
        /// outcome, and reporting it as `exit_code` would be a fabricated fact.
        /// Unix already reports `None` here because a signalled process has no
        /// exit code, so both platforms now agree.
        pub(crate) async fn terminate_and_reap(&mut self, _grace: Duration) -> TreeConvergence {
            match self.child.start_kill() {
                Ok(()) => TreeConvergence::converged(None),
                Err(error) => TreeConvergence {
                    status: None,
                    signal_error: Some(error.to_string()),
                    wait_error: None,
                },
            }
        }
    }
}

/// Compiled targets with no tree-ownership story at all. The runner still
/// validates the request first and then refuses, so an unsupported target is a
/// typed refusal rather than a silent partial execution.
#[cfg(not(any(unix, windows)))]
mod imp {
    use super::*;
    use crate::RunnerError;

    pub(crate) struct ProcessTree;

    impl ProcessTree {
        pub(crate) fn spawn_platform(_command: Command) -> Result<Self, RunnerError> {
            Err(RunnerError::UnsupportedPlatform)
        }

        pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
            None
        }

        pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
            None
        }

        pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
            None
        }

        pub(crate) async fn wait(&mut self) -> std::io::Result<ExitStatus> {
            Err(std::io::Error::other(
                RunnerError::UnsupportedPlatform.to_string(),
            ))
        }

        pub(crate) async fn converge_after_leader_exit(&mut self) -> TreeConvergence {
            TreeConvergence::converged(None)
        }

        pub(crate) async fn terminate_and_reap(&mut self, _grace: Duration) -> TreeConvergence {
            TreeConvergence::converged(None)
        }
    }
}

pub(crate) use imp::ProcessTree;
