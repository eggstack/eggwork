//! Windows process-tree lifecycle proofs for the runner.
//!
//! These run natively on Windows and nowhere else: they assert the Job Object
//! behaviour that cannot be observed from a Unix host.
//!
//! # Fixtures are `cmd.exe` batch files, and that is deliberate
//!
//! The first version of this suite drove PowerShell and eleven of sixteen tests
//! failed on `windows-latest` (run `37411582121`) — not because the backend
//! misbehaved, but because sixteen concurrent PowerShell startups on a hosted
//! runner take longer than the deadlines under test. `cmd.exe` starts in
//! milliseconds and expresses everything these proofs need: file writes,
//! loops, environment dumps, stdin reads, exit codes, and child processes.
//! PowerShell would have made the evidence weaker by making it timing-shaped.
//!
//! # How "no process survived" is proven
//!
//! There is no safe-Rust process-liveness API in this workspace, so a fixture
//! appends its own heartbeat on a ~1 s cadence and the test samples that file
//! *after* the runner has returned. Growth means a descendant is still running;
//! a frozen file is the evidence that it is not. That is stronger evidence than
//! a name-based enumeration sweep, which races with process exit and says
//! nothing about *when* the process died.
//!
//! `assert_frozen` first asserts the file is non-empty, so a fixture that never
//! ran can never make the proof pass vacuously, and it then waits longer than
//! two beats and re-samples.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eggwork_core::{ExecutionState, OverflowPolicy, RelativePath, Requirement};
use eggwork_runner::{
    LocalProcessRunner, NoExecutionSetup, ResourceSetupOutcome, ResourceSetupRequest, RunnerError,
    RunnerRequest, RunnerResult, SandboxOutcome, SandboxRequest, StdinPolicy, TerminationReason,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Longer than the heartbeat cadence by more than two beats.
const FREEZE_WINDOW: Duration = Duration::from_millis(2500);
/// Deadlines under test are short on purpose, but not so short that a hosted
/// runner's process startup decides the result.
const INTERRUPT_AFTER: Duration = Duration::from_millis(3000);
/// Generous ceiling for fixtures that are supposed to finish on their own.
const PATIENCE: Duration = Duration::from_secs(120);

/// The environment every execution child is guaranteed to see. Kept in sync
/// with `baseline_child_environment` in the crate; a mismatch is a contract
/// change, not a test fix.
const BASELINE_KEYS: [&str; 8] = [
    "CI",
    "GIT_TERMINAL_PROMPT",
    "NO_COLOR",
    "PAGER",
    "PATH",
    "SystemRoot",
    "TERM",
    "windir",
];

fn cmd() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
}

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            temp: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().to_path_buf()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    /// Write a batch fixture and return argv that runs it.
    ///
    /// A batch file rather than a `/C` one-liner: `cmd /C "echo a & echo b"`
    /// echoes a trailing space before `&`, and quoting grows teeth. A file has
    /// none of that.
    fn batch(&self, name: &str, body: &str) -> Vec<String> {
        let path = self.path(name);
        fs::write(&path, body).expect("write batch fixture");
        vec![cmd(), "/C".to_owned(), path.to_string_lossy().into_owned()]
    }

    /// argv for a batch fixture path, used when a parent has to launch it.
    fn batch_argv(path: &Path) -> Vec<String> {
        vec![cmd(), "/C".to_owned(), path.to_string_lossy().into_owned()]
    }

    /// A fixture that appends one heartbeat line to `file` about once a second,
    /// forever. `announce` writes the file immediately so a parent can wait for
    /// the descendant to be provably alive rather than merely launched.
    fn heartbeat_body(file: &Path, announce: bool) -> String {
        let target = file.display();
        let announce = if announce {
            format!(">\"{target}\" echo started\n")
        } else {
            String::new()
        };
        format!(
            "@echo off\n\
             {announce}:loop\n\
             >>\"{target}\" echo beat\n\
             ping -n 2 127.0.0.1 >nul\n\
             goto loop\n"
        )
    }

    /// A parent that launches `child` detached, waits until the descendant
    /// proves it is alive, then exits with `hold_seconds` delay (0 = exit
    /// immediately). Bounded by `deadline_polls` so a broken fixture fails
    /// loudly instead of hanging.
    fn spawner_body(child: &Path, ready: &Path, deadline_polls: u32) -> String {
        format!(
            "@echo off\n\
             start \"\" /B cmd /C \"{child}\"\n\
             set /a polls=0\n\
             :wait\n\
             if exist \"{ready}\" goto ready\n\
             set /a polls+=1\n\
             if %polls% geq {deadline_polls} exit /B 9\n\
             ping -n 2 127.0.0.1 >nul\n\
             goto wait\n\
             :ready\n\
             exit /B 0\n",
            child = child.display(),
            ready = ready.display(),
            deadline_polls = deadline_polls,
        )
    }

    fn request(&self, argv: Vec<String>) -> RunnerRequest {
        let mut request = RunnerRequest::new(argv, self.root());
        request.set_timeout(PATIENCE);
        request.set_output_policy(1024 * 1024, 4096, OverflowPolicy::Truncate);
        request
    }
}

async fn run(request: RunnerRequest) -> Result<RunnerResult, RunnerError> {
    run_with_cancellation(request, CancellationToken::new()).await
}

async fn run_with_cancellation(
    request: RunnerRequest,
    cancellation: CancellationToken,
) -> Result<RunnerResult, RunnerError> {
    let runner = LocalProcessRunner::default();
    let (tx, _rx) = mpsc::channel(8);
    runner.run(request, cancellation, tx).await
}

async fn run_with(
    runner: LocalProcessRunner,
    request: RunnerRequest,
) -> Result<RunnerResult, RunnerError> {
    let (tx, _rx) = mpsc::channel(8);
    runner.run(request, CancellationToken::new(), tx).await
}

fn size_of(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Assert the fixture stopped beating: the file must exist and be non-empty (so
/// the proof is not vacuous) and must not grow across a window wider than two
/// beats.
async fn assert_frozen(path: &Path, context: &str) {
    let first = size_of(path);
    assert!(
        first > 0,
        "{context}: fixture never beat, so this proof would be vacuous"
    );
    tokio::time::sleep(FREEZE_WINDOW).await;
    let second = size_of(path);
    assert_eq!(
        first, second,
        "{context}: heartbeat grew from {first} to {second} bytes after the runner returned"
    );
}

fn assert_no_cleanup_warning(result: &RunnerResult, context: &str) {
    assert_eq!(
        result.cleanup.process_group_signal_error, None,
        "{context}: tree termination reported a failure"
    );
    assert_eq!(result.cleanup.wait_error, None, "{context}: reap failed");
    assert_eq!(
        result.cleanup.stdin_error, None,
        "{context}: stdin writer failed"
    );
    assert_eq!(
        result.execution_result().cleanup_warning,
        None,
        "{context}: convergence surfaced as a cleanup warning"
    );
}

/// Windows has two spellings of the same path: `fs::canonicalize` returns the
/// verbatim `\\?\` form and a shell prints the plain one. Comparing raw strings
/// would fail on spelling, not on behaviour.
fn normalise_path(value: &str) -> String {
    value
        .trim()
        .trim_start_matches(r#"\\?\"#)
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
}

fn stdout_of(result: &RunnerResult) -> String {
    String::from_utf8_lossy(&result.stdout.head).into_owned()
}

/// §9.1 — an ordinary target runs, streams, and reports its exit status.
#[tokio::test]
async fn direct_exit_captures_output_and_reports_status() {
    let fixture = Fixture::new();
    let argv = fixture.batch(
        "direct.cmd",
        "@echo off\n\
         echo out-marker\n\
         >&2 echo err-marker\n\
         exit /B 3\n",
    );
    let result = run(fixture.request(argv)).await.expect("execution ran");
    assert_eq!(result.termination, TerminationReason::Exited);
    assert_eq!(result.exit_code, Some(3));
    assert_eq!(result.stdout.head, b"out-marker\r\n");
    assert_eq!(result.stderr.head, b"err-marker\r\n");
    assert_no_cleanup_warning(&result, "direct exit");
}

/// §9.2 — a descendant created in the target's first moments is owned, even
/// when the leader exits immediately afterwards.
#[tokio::test]
async fn a_descendant_created_at_startup_is_owned_after_the_leader_exits() {
    let fixture = Fixture::new();
    let beat = fixture.path("descendant.beat");
    let child = fixture.path("descendant.cmd");
    fs::write(&child, Fixture::heartbeat_body(&beat, true)).expect("write descendant");
    let argv = fixture.batch("spawner.cmd", &Fixture::spawner_body(&child, &beat, 30));
    let result = run(fixture.request(argv)).await.expect("execution ran");
    assert_eq!(result.termination, TerminationReason::Exited);
    assert_eq!(result.exit_code, Some(0));
    assert_no_cleanup_warning(&result, "leader exit with descendant");
    assert_frozen(&beat, "descendant created at startup").await;
}

/// §9.3 — timeout termination converges the whole tree.
#[tokio::test]
async fn timeout_terminates_the_whole_tree() {
    let fixture = Fixture::new();
    let leader_beat = fixture.path("leader.beat");
    let descendant_beat = fixture.path("descendant.beat");
    let child = fixture.path("descendant.cmd");
    fs::write(&child, Fixture::heartbeat_body(&descendant_beat, true)).expect("write descendant");
    let leader = fixture.path("leader.cmd");
    fs::write(
        &leader,
        format!(
            "@echo off\n\
             start \"\" /B cmd /C \"{child}\"\n\
             :loop\n\
             >>\"{beat}\" echo beat\n\
             ping -n 2 127.0.0.1 >nul\n\
             goto loop\n",
            child = child.display(),
            beat = leader_beat.display(),
        ),
    )
    .expect("write leader");

    let mut request = fixture.request(Fixture::batch_argv(&leader));
    request.set_timeout(INTERRUPT_AFTER);
    let result = run(request).await.expect("execution ran");
    assert_eq!(result.termination, TerminationReason::TimedOut);
    assert_no_cleanup_warning(&result, "timeout");
    assert_frozen(&leader_beat, "leader after timeout").await;
    assert_frozen(&descendant_beat, "descendant after timeout").await;
}

/// §9.4 — cancellation termination converges the whole tree.
#[tokio::test]
async fn cancellation_terminates_the_whole_tree() {
    let fixture = Fixture::new();
    let beat = fixture.path("leader.beat");
    let leader = fixture.path("leader.cmd");
    fs::write(&leader, Fixture::heartbeat_body(&beat, false)).expect("write leader");

    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(INTERRUPT_AFTER).await;
        token.cancel();
    });
    let result = run_with_cancellation(fixture.request(Fixture::batch_argv(&leader)), cancellation)
        .await
        .expect("execution ran");
    assert_eq!(result.termination, TerminationReason::Cancelled);
    assert_no_cleanup_warning(&result, "cancellation");
    assert_frozen(&beat, "leader after cancellation").await;
}

/// §9.5 — output-limit termination converges the whole tree.
#[tokio::test]
async fn output_limit_terminates_the_whole_tree() {
    let fixture = Fixture::new();
    let beat = fixture.path("leader.beat");
    let leader = fixture.path("leader.cmd");
    // No sleep in the loop: the flood has to outrun the capture limit quickly
    // so this test measures output-limit termination, not scheduling.
    fs::write(
        &leader,
        format!(
            "@echo off\n\
             :loop\n\
             >>\"{beat}\" echo beat\n\
             for /L %%i in (1,1,400) do @echo 0123456789012345678901234567890123456789\n\
             goto loop\n",
            beat = beat.display(),
        ),
    )
    .expect("write leader");

    let mut request = fixture.request(Fixture::batch_argv(&leader));
    // `Terminate`, not `Truncate`: a truncating overflow is *supposed* to keep
    // the target running, so asking it to truncate and then expecting
    // `OutputLimit` would have been testing the fixture's wish, not the runner.
    request.set_output_policy(16 * 1024, 512, OverflowPolicy::Terminate);
    let result = run(request).await.expect("execution ran");
    assert_eq!(result.termination, TerminationReason::OutputLimit);
    assert_no_cleanup_warning(&result, "output limit");
    assert_frozen(&beat, "leader after output limit").await;
}

/// §9.6 — a leader that exits while a descendant runs does not release the
/// runner, and the descendant is gone when it does.
#[tokio::test]
async fn leader_exit_never_releases_a_remaining_descendant() {
    let fixture = Fixture::new();
    let descendant = fixture.path("descendant.beat");
    let child = fixture.path("descendant.cmd");
    fs::write(&child, Fixture::heartbeat_body(&descendant, true)).expect("write descendant");
    // The leader exits as soon as the descendant is alive, which is the case a
    // job-object backend must not mistake for "the whole tree is finished".
    let argv = fixture.batch(
        "spawner.cmd",
        &Fixture::spawner_body(&child, &descendant, 30),
    );
    let result = run(fixture.request(argv)).await.expect("execution ran");
    assert_eq!(result.termination, TerminationReason::Exited);
    assert_eq!(result.exit_code, Some(0));
    assert_frozen(&descendant, "descendant after leader exit").await;
}

/// The leader's own exit classification is decided by its exit status, not by
/// how long its descendants took to converge.
#[tokio::test]
async fn leader_exit_status_is_not_rewritten_by_descendant_termination() {
    let fixture = Fixture::new();
    let descendant = fixture.path("descendant.beat");
    let child = fixture.path("descendant.cmd");
    fs::write(&child, Fixture::heartbeat_body(&descendant, true)).expect("write descendant");
    let argv = fixture.batch(
        "spawner.cmd",
        &Fixture::spawner_body(&child, &descendant, 30),
    );
    let result = run(fixture.request(argv)).await.expect("execution ran");
    let execution = result.execution_result();
    assert_eq!(execution.exit_code, Some(0));
    assert_eq!(execution.state, ExecutionState::Succeeded);
    assert_eq!(execution.cleanup_warning, None);
}

/// §9.7 — the working directory is the requested one, and Eggwork makes no
/// filesystem isolation claim on Windows.
#[tokio::test]
async fn working_directory_is_honoured_and_no_isolation_is_claimed() {
    let fixture = Fixture::new();
    let work = fixture.root().join("work");
    let outside = fixture.path("outside.txt");
    fs::create_dir_all(&work).expect("work dir");
    let argv = fixture.batch(
        "cwd.cmd",
        &format!(
            "@echo off\n\
             echo written> inside.txt\n\
             cd\n\
             echo no-isolation-claim> \"{outside}\"\n",
            outside = outside.display(),
        ),
    );
    let mut request = fixture.request(argv);
    request.set_working_directory(Some(
        RelativePath::new("work".to_owned()).expect("relative"),
    ));

    let result = run(request).await.expect("execution ran");
    assert_eq!(result.exit_code, Some(0));

    let reported = stdout_of(&result);
    let expected = fs::canonicalize(&work).expect("canonical work dir");
    // `fs::canonicalize` returns the verbatim `\\?\` form on Windows and `cmd`
    // prints the plain form, so both sides are normalised before comparing.
    assert!(
        normalise_path(&reported) == normalise_path(&expected.to_string_lossy()),
        "target ran somewhere other than the requested working directory: {reported:?}"
    );
    assert!(
        work.join("inside.txt").exists(),
        "target could not write inside its working directory"
    );
    // Windows has no filesystem confinement here, and the receipt must not
    // pretend otherwise: a successful write outside the execution root is the
    // documented behaviour, and `NotRequested` is what keeps it honest.
    assert!(
        outside.exists(),
        "unexpected confinement: Eggwork does not confine the Windows filesystem"
    );
    assert_eq!(result.setup.sandbox, SandboxOutcome::NotRequested);
}

/// §9.8 — stdin bytes are delivered and the writer is closed.
#[tokio::test]
async fn stdin_is_delivered_and_closed() {
    let fixture = Fixture::new();
    let argv = fixture.batch(
        "stdin.cmd",
        "@echo off\n\
         set /p line=\n\
         echo got=[%line%]\n",
    );
    let mut request = fixture.request(argv);
    request.set_stdin_policy(StdinPolicy::Bytes(b"piped-input\r\n".to_vec()));
    let result = run(request).await.expect("execution ran");
    assert_eq!(result.exit_code, Some(0));
    let stdout = stdout_of(&result);
    assert!(
        stdout.contains("got=[piped-input]"),
        "stdin bytes never reached the target: {stdout:?}"
    );
    assert_no_cleanup_warning(&result, "stdin");
}

/// §9.9 — a spawn failure is typed, and nothing is left running.
#[tokio::test]
async fn spawn_failure_is_typed_and_leaves_nothing_running() {
    let fixture = Fixture::new();
    let marker = fixture.path("spawn-failure-marker");
    let request = fixture.request(vec![
        fixture
            .path("definitely-missing.exe")
            .to_string_lossy()
            .into_owned(),
    ]);
    // The program does not exist, so the platform must fail while creating the
    // tree owner and report a typed spawn error: no job, no process group, and
    // nothing to clean up.
    assert!(matches!(run(request).await, Err(RunnerError::Spawn(_))));
    assert!(!marker.exists());
}

/// §9.10 — descendant output reaches the execution's captured streams, which is
/// what proves pipe handles are inherited across the tree.
#[tokio::test]
async fn descendant_output_reaches_the_captured_streams() {
    let fixture = Fixture::new();
    let child = fixture.path("descendant.cmd");
    fs::write(
        &child,
        "@echo off\n\
         echo descendant-stdout\n\
         >&2 echo descendant-stderr\n",
    )
    .expect("write descendant");
    // A nested `cmd /C` is a real second process that inherits the runner's
    // stdout and stderr handles. `call` would not do: a batch file invoked with
    // `call` runs inside the same process.
    let argv = fixture.batch(
        "leader.cmd",
        &format!(
            "@echo off\r\ncmd /C \"{child}\"\r\n",
            child = child.display()
        ),
    );
    let result = run(fixture.request(argv)).await.expect("execution ran");
    assert_eq!(result.exit_code, Some(0));
    let stdout = stdout_of(&result);
    let stderr = String::from_utf8_lossy(&result.stderr.head).into_owned();
    assert!(
        stdout.contains("descendant-stdout"),
        "descendant stdout was not captured: {stdout:?}"
    );
    assert!(
        stderr.contains("descendant-stderr"),
        "descendant stderr was not captured: {stderr:?}"
    );
}

/// §9.11 — the child environment is the documented Windows baseline: every
/// baseline key is present, and nothing from the daemon's own ambient
/// environment leaks through.
#[tokio::test]
async fn environment_is_the_documented_baseline_and_leaks_nothing() {
    let fixture = Fixture::new();
    let argv = fixture.batch("env.cmd", "@echo off\nset\n");
    let result = run(fixture.request(argv)).await.expect("execution ran");
    assert_eq!(result.exit_code, Some(0));
    let reported = stdout_of(&result);

    let child_keys: Vec<&str> = reported
        .lines()
        // `set` also prints cmd's own drive-current-directory pseudo variables,
        // which start with `=` and carry no key.
        .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim()))
        .filter(|key| !key.is_empty())
        .collect();
    // Anything outside this set came from the ambient environment, which is the
    // thing under test. `cmd.exe` synthesises its own variables at startup, so
    // they are expected and are not leaks: the previous version of this test
    // failed on exactly `COMSPEC`, `PATHEXT`, and `PROMPT`, which is the
    // interpreter talking, not the runner leaking.
    let synthesised_by_cmd: [&str; 3] = ["COMSPEC", "PATHEXT", "PROMPT"];

    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    for key in BASELINE_KEYS {
        assert!(
            child_keys.contains(&key),
            "baseline key {key} missing from the child environment: {child_keys:?}"
        );
    }
    assert!(
        reported.contains(&format!("PATH={system_root}\\System32;{system_root}")),
        "PATH was not the documented Windows baseline: {reported:?}"
    );

    // `env_clear` has to keep the daemon's environment out. Rather than
    // mutating this process's environment (which needs `unsafe`, and this crate
    // denies it), the test compares the keys the target actually received
    // against the keys that exist in the ambient environment right now.
    let leaked: Vec<&str> = child_keys
        .iter()
        .copied()
        .filter(|key| std::env::var_os(key).is_some())
        .filter(|key| !BASELINE_KEYS.contains(key))
        .filter(|key| !synthesised_by_cmd.contains(key))
        .collect();
    assert!(
        leaked.is_empty(),
        "ambient daemon environment reached the target: {leaked:?} (child saw {child_keys:?})"
    );
}

/// §9.12 — a bare program name resolves against the baseline PATH.
#[tokio::test]
async fn bare_program_names_resolve_against_the_baseline_path() {
    let fixture = Fixture::new();
    let request = fixture.request(vec![
        "cmd".to_owned(),
        "/C".to_owned(),
        "echo resolved-via-path".to_owned(),
    ]);
    let result = run(request).await.expect("execution ran");
    assert_eq!(result.exit_code, Some(0));
    assert!(result.stdout.head.starts_with(b"resolved-via-path"));
}

/// §9.13 — a required filesystem isolation request is refused before spawn,
/// because Windows has no Landlock.
#[tokio::test]
async fn required_isolation_is_refused_before_the_target_runs() {
    let fixture = Fixture::new();
    let marker = fixture.path("required-isolation-marker");
    let argv = fixture.batch(
        "marker.cmd",
        &format!(
            "@echo off\ntype nul> \"{marker}\"\n",
            marker = marker.display()
        ),
    );
    let mut request = fixture.request(argv);
    request.set_sandbox_request(SandboxRequest::Required {
        profile: "filesystem.workspace-rw.v1".to_owned(),
    });
    assert!(matches!(
        run_with(LocalProcessRunner::new(NoExecutionSetup), request).await,
        Err(RunnerError::RequiredSetupUnavailable)
    ));
    assert!(
        !marker.exists(),
        "a refused required isolation request still ran the target"
    );
}

/// §9.14 — required resources are refused before spawn for the same reason, and
/// a best-effort request is reported as not applied rather than as enforced.
#[tokio::test]
async fn required_resources_are_refused_and_best_effort_is_reported() {
    let fixture = Fixture::new();
    let marker = fixture.path("required-resource-marker");
    let required_argv = fixture.batch(
        "marker.cmd",
        &format!(
            "@echo off\ntype nul> \"{marker}\"\n",
            marker = marker.display()
        ),
    );
    let mut required = fixture.request(required_argv);
    required.set_resource_setup_request(ResourceSetupRequest {
        memory_bytes: Requirement::NotRequested,
        cpu_millis: Requirement::NotRequested,
        pids: Requirement::Required(8),
    });
    assert!(matches!(
        run_with(LocalProcessRunner::new(NoExecutionSetup), required).await,
        Err(RunnerError::RequiredSetupUnavailable)
    ));
    assert!(
        !marker.exists(),
        "a refused required resource request still ran the target"
    );

    let best_effort_argv = fixture.batch("ok.cmd", "@echo off\nexit /B 0\n");
    let mut best_effort = fixture.request(best_effort_argv);
    best_effort.set_resource_setup_request(ResourceSetupRequest {
        memory_bytes: Requirement::BestEffort(64 * 1024 * 1024),
        cpu_millis: Requirement::NotRequested,
        pids: Requirement::NotRequested,
    });
    let result = run_with(LocalProcessRunner::new(NoExecutionSetup), best_effort)
        .await
        .expect("best-effort execution ran");
    assert_eq!(result.exit_code, Some(0));
    assert!(matches!(
        result.setup.resources,
        ResourceSetupOutcome::NotApplied { .. }
    ));
}

/// Two fixtures, two runner instances, two workspaces, two executions at once:
/// the shape that failed with `Text file busy` in CI.
#[tokio::test]
async fn concurrent_fixtures_both_execute_concurrently() {
    let first = Fixture::new();
    let second = Fixture::new();
    let first_beat = first.path("leader.beat");
    let second_beat = second.path("leader.beat");
    let first_argv = first.batch("leader.cmd", &Fixture::heartbeat_body(&first_beat, false));
    let second_argv = second.batch("leader.cmd", &Fixture::heartbeat_body(&second_beat, false));

    let mut first_request = first.request(first_argv);
    first_request.set_timeout(INTERRUPT_AFTER);
    let mut second_request = second.request(second_argv);
    second_request.set_timeout(INTERRUPT_AFTER);

    let (first_result, second_result) = tokio::join!(run(first_request), run(second_request));
    let first_result = first_result.expect("first fixture executed");
    let second_result = second_result.expect("second fixture executed");
    assert_eq!(first_result.termination, TerminationReason::TimedOut);
    assert_eq!(second_result.termination, TerminationReason::TimedOut);
    assert_no_cleanup_warning(&first_result, "first fixture");
    assert_no_cleanup_warning(&second_result, "second fixture");
    assert_frozen(&first_beat, "first fixture after timeout").await;
    assert_frozen(&second_beat, "second fixture after timeout").await;
}

/// Tree bugs are timing bugs, so a single green termination is not evidence.
#[tokio::test]
async fn repeated_timeout_termination_converges_every_time() {
    for iteration in 0..3 {
        let fixture = Fixture::new();
        let beat = fixture.path("leader.beat");
        let leader = fixture.path("leader.cmd");
        fs::write(&leader, Fixture::heartbeat_body(&beat, false)).expect("write leader");
        let mut request = fixture.request(Fixture::batch_argv(&leader));
        request.set_timeout(INTERRUPT_AFTER);
        let result = run(request).await.expect("execution ran");
        assert_eq!(
            result.termination,
            TerminationReason::TimedOut,
            "iteration {iteration}"
        );
        assert_no_cleanup_warning(&result, &format!("iteration {iteration}"));
        assert_frozen(&beat, &format!("iteration {iteration}")).await;
    }
}
