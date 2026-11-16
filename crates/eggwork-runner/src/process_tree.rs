//! Platform-specific process-tree owner for the shared runner lifecycle.
//!
//! Unix uses process groups; Windows assigns the suspended child to a Job
//! Object before resume. The runner releases permits after convergence. Windows
//! uses kernel termination and pipe EOF as evidence; kill-on-close is a backstop.

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
        ///
        /// Linux-only for the same reason its call sites are: on the other
        /// Unix targets the handshake has no refusal path, and leaving the
        /// method defined made the macOS build fail `clippy -D warnings` on a
        /// dead-code error.
        #[cfg(target_os = "linux")]
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

        /// Ask the group to stop, wait `grace`, then force termination and reap
        /// the leader. The bound applies to both waits.
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

        /// Terminate descendants left in the Job Object after the leader exits.
        /// Pipe EOF provides convergence evidence; kill-on-close is the backstop.
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

        /// Terminate the job's leader and descendants in one call.
        ///
        /// Do not report the leader's exit status: it describes Eggwork's kill,
        /// not the target's outcome.
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
