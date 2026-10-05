//! Windows process-tree lifecycle proofs for the runner.
//!
//! These run natively on Windows and nowhere else: they assert the Job Object
//! behaviour that cannot be observed from a Unix host. Fixtures are Windows
//! programs (`cmd.exe` and `powershell.exe`), and every tree claim is proven by
//! a fixture that reports its own liveness.
//!
//! # How "no process survived" is proven
//!
//! There is no safe-Rust process-liveness API in this workspace, so the tests
//! do the honest thing: a fixture writes its own heartbeat on a cadence, and
//! the test samples that file *after* the runner has returned. Growth means a
//! descendant is still running; a frozen file is the evidence that it is not.
//! That is stronger evidence than a name-based enumeration sweep, which races
//! with process exit and says nothing about *when* the process died.
//!
//! `assert_frozen` waits longer than several beats and re-samples, so a
//! descendant that dies late still fails the assertion instead of being missed,
//! and it first asserts the file is non-empty, so a fixture that never ran can
//! never make the proof pass vacuously.

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

const BEAT_MILLIS: u64 = 50;
/// Long enough to see a live writer append several beats, short enough to keep
/// the suite quick.
const FREEZE_WINDOW: Duration = Duration::from_millis(600);

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

fn powershell() -> String {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

fn cmd() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
}

/// `cmd.exe` redirection needs a quoted destination: Windows temp paths live
/// under the user profile, which routinely contains a space.
fn cmd_path(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// A PowerShell single-quoted string literal. Windows temp paths routinely
/// contain spaces and can contain apostrophes, so quoting cannot be skipped.
fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// argv that runs `powershell.exe` with a fixed, profile-free front end.
fn powershell_argv(tail: Vec<String>) -> Vec<String> {
    vec![
        powershell(),
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
    ]
    .into_iter()
    .chain(tail)
    .collect()
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

    fn write_script(&self, name: &str, body: &str) -> Vec<String> {
        let path = self.path(name);
        fs::write(&path, body).expect("write fixture script");
        powershell_argv(vec![
            "-File".to_owned(),
            path.to_string_lossy().into_owned(),
        ])
    }

    /// A fixture that appends a beat to `file` forever. `announce` records a
    /// first line before looping, which is how a parent learns the descendant is
    /// actually running rather than merely launched.
    fn heartbeat_body(file: &Path, announce: bool) -> String {
        format!(
            "$f = {file}\n\
             if ({announce}) {{ Set-Content -Path $f -Value 'started' }}\n\
             while ($true) {{ Add-Content -Path $f -Value 'beat'; Start-Sleep -Milliseconds {beat} }}\n",
            file = ps_literal(&file.to_string_lossy()),
            announce = announce,
            beat = BEAT_MILLIS,
        )
    }

    /// Write a heartbeat fixture script and return its path, because a spawner
    /// fixture needs the path to hand to `Start-Process`.
    fn heartbeat_script(&self, name: &str, file: &Path, announce: bool) -> PathBuf {
        let body = Self::heartbeat_body(file, announce);
        self.write_script(name, &body);
        self.path(name)
    }

    /// A fixture that starts `child_script` detached, waits until the descendant
    /// proves it is alive, and then exits after `hold_millis`.
    ///
    /// `hold_millis: 0` is the interesting case: the leader exits while an owned
    /// descendant is still running.
    fn spawner_script(
        &self,
        name: &str,
        child_script: &Path,
        ready_file: &Path,
        hold_millis: u64,
    ) -> Vec<String> {
        let body = Self::spawner_body(child_script, ready_file, hold_millis);
        self.write_script(name, &body)
    }

    fn spawner_body(child_script: &Path, ready_file: &Path, hold_millis: u64) -> String {
        // `-ArgumentList` elements are joined with spaces and are *not* quoted
        // for you, so the child command line is assembled explicitly.
        format!(
            "$child = {child}\n\
             $line = '-NoProfile -NonInteractive -File \"' + $child + '\"'\n\
             Start-Process -FilePath {shell} -ArgumentList $line -WindowStyle Hidden\n\
             $deadline = (Get-Date).AddSeconds(15)\n\
             while (-not (Test-Path {ready})) {{\n\
             \x20   if ((Get-Date) -gt $deadline) {{ exit 9 }}\n\
             \x20   Start-Sleep -Milliseconds {beat}\n\
             }}\n\
             Start-Sleep -Milliseconds {hold}\n",
            child = ps_literal(&child_script.to_string_lossy()),
            shell = ps_literal(&powershell()),
            ready = ps_literal(&ready_file.to_string_lossy()),
            beat = BEAT_MILLIS,
            hold = hold_millis,
        )
    }

    /// A fixture that spawns `child_script` through PowerShell's call operator
    /// as a *separate process*, so the child inherits the runner's stdout and
    /// stderr pipes.
    fn inherited_stdio_body(child_script: &Path) -> String {
        format!(
            "& {shell} -NoProfile -NonInteractive -File {child}\n",
            shell = ps_literal(&powershell()),
            child = ps_literal(&child_script.to_string_lossy()),
        )
    }

    fn request(&self, argv: Vec<String>) -> RunnerRequest {
        let mut request = RunnerRequest::new(argv, self.root());
        request.set_timeout(Duration::from_secs(60));
        request.set_output_policy(1024 * 1024, 4096, OverflowPolicy::Truncate);
        request
    }

    fn cmd_request(&self, script: &str) -> RunnerRequest {
        self.request(vec![cmd(), "/C".to_owned(), script.to_owned()])
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

/// Compare two Windows paths without letting the two spellings of the same
/// path (`\\?\C:\x` from the filesystem API, `C:\x` from a shell) fail a
/// test for a spelling reason.
fn same_path(reported: &str, expected: &Path) -> bool {
    fn normalise(value: &str) -> String {
        value
            .trim()
            .trim_start_matches(r#"\\?\"#)
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    }
    normalise(reported) == normalise(&expected.to_string_lossy())
}

fn size_of(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Assert the fixture stopped beating: the file must exist and be non-empty (so
/// the proof is not vacuous) and must not grow across a window wider than
/// several beats.
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

/// §9.1 — an ordinary target runs, streams, and reports its exit status.
#[tokio::test]
async fn direct_exit_captures_output_and_reports_status() {
    let fixture = Fixture::new();
    let request = fixture.cmd_request("echo out-marker & echo err-marker 1>&2 & exit /B 3");
    let result = run(request).await.expect("execution ran");
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
    let child = fixture.heartbeat_script("descendant.ps1", &beat, true);
    let request = fixture.request(fixture.spawner_script("spawner.ps1", &child, &beat, 0));
    let result = run(request).await.expect("execution ran");
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
    let child = fixture.heartbeat_script("child.ps1", &descendant_beat, true);
    let body = format!(
        "$f = {leader}\n\
         $child = {child}\n\
         $line = '-NoProfile -NonInteractive -File \"' + $child + '\"'\n\
         Start-Process -FilePath {shell} -ArgumentList $line -WindowStyle Hidden\n\
         while ($true) {{ Add-Content -Path $f -Value 'beat'; Start-Sleep -Milliseconds {beat} }}\n",
        leader = ps_literal(&leader_beat.to_string_lossy()),
        child = ps_literal(&child.to_string_lossy()),
        shell = ps_literal(&powershell()),
        beat = BEAT_MILLIS,
    );
    let mut request = fixture.request(fixture.write_script("leader.ps1", &body));
    request.set_timeout(Duration::from_millis(2500));

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
    let body = format!(
        "$f = {file}\n\
         while ($true) {{ Add-Content -Path $f -Value 'beat'; Start-Sleep -Milliseconds {beat} }}\n",
        file = ps_literal(&beat.to_string_lossy()),
        beat = BEAT_MILLIS,
    );
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        token.cancel();
    });
    let result = run_with_cancellation(
        fixture.request(fixture.write_script("leader.ps1", &body)),
        cancellation,
    )
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
    let body = format!(
        "$f = {file}\n\
         while ($true) {{\n\
         \x20   Add-Content -Path $f -Value 'beat'\n\
         \x20   Write-Output ('x' * 1024)\n\
         \x20   Start-Sleep -Milliseconds {beat}\n\
         }}\n",
        file = ps_literal(&beat.to_string_lossy()),
        beat = BEAT_MILLIS,
    );
    let mut request = fixture.request(fixture.write_script("leader.ps1", &body));
    request.set_output_policy(16 * 1024, 512, OverflowPolicy::Truncate);

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
    let child = fixture.heartbeat_script("child.ps1", &descendant, true);
    // The leader exits as soon as the descendant is alive, which is the case a
    // job-object backend must not mistake for "the whole tree is finished".
    let request = fixture.request(fixture.spawner_script("spawner.ps1", &child, &descendant, 0));
    let result = run(request).await.expect("execution ran");
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
    let child = fixture.heartbeat_script("child.ps1", &descendant, true);
    let request = fixture.request(fixture.spawner_script("spawner.ps1", &child, &descendant, 0));
    let result = run(request).await.expect("execution ran");
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
    let body = format!(
        "Set-Content -Path 'inside.txt' -Value 'written'\n\
         Write-Output (Get-Location).Path\n\
         Set-Content -Path {outside} -Value 'no-isolation-claim'\n",
        outside = ps_literal(&outside.to_string_lossy()),
    );
    let mut request = fixture.request(fixture.write_script("cwd.ps1", &body));
    request.set_working_directory(Some(
        RelativePath::new("work".to_owned()).expect("relative"),
    ));

    let result = run(request).await.expect("execution ran");
    assert_eq!(result.exit_code, Some(0));

    let reported = String::from_utf8_lossy(&result.stdout.head).into_owned();
    let expected = fs::canonicalize(&work).expect("canonical work dir");
    assert!(
        same_path(&reported, &expected),
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
    let body = "$input = [Console]::In.ReadToEnd()\n\
                Write-Output (\"len=\" + $input.Length)\n\
                Write-Output $input\n";
    let mut request = fixture.request(fixture.write_script("stdin.ps1", body));
    request.set_stdin_policy(StdinPolicy::Bytes(b"piped-input".to_vec()));
    let result = run(request).await.expect("execution ran");
    assert_eq!(result.exit_code, Some(0));
    let stdout = String::from_utf8_lossy(&result.stdout.head).into_owned();
    assert!(
        stdout.contains("len=11"),
        "stdin bytes never arrived: {stdout:?}"
    );
    assert!(
        stdout.contains("piped-input"),
        "stdin content was not echoed back: {stdout:?}"
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
    let child = fixture.path("child.ps1");
    fs::write(
        &child,
        "Write-Output 'descendant-stdout'\n\
         [Console]::Error.WriteLine('descendant-stderr')\n",
    )
    .expect("write child script");
    let body = Fixture::inherited_stdio_body(&child);
    let result = run(fixture.request(fixture.write_script("leader.ps1", &body)))
        .await
        .expect("execution ran");
    assert_eq!(result.exit_code, Some(0));
    let stdout = String::from_utf8_lossy(&result.stdout.head).into_owned();
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
    let body = "Get-ChildItem Env: | ForEach-Object { $_.Name + '=' + $_.Value } | Sort-Object\n";
    let result = run(fixture.request(fixture.write_script("env.ps1", body)))
        .await
        .expect("execution ran");
    assert_eq!(result.exit_code, Some(0));
    let reported = String::from_utf8_lossy(&result.stdout.head).into_owned();

    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    for key in BASELINE_KEYS {
        assert!(
            reported.contains(&format!("{key}=")),
            "baseline key {key} missing from the child environment: {reported:?}"
        );
    }
    assert!(
        reported.contains(&format!("PATH={system_root}\\System32;{system_root}")),
        "PATH was not the documented Windows baseline: {reported:?}"
    );
    assert!(
        reported.contains(&format!("SystemRoot={system_root}")),
        "SystemRoot is required by the loader and was missing: {reported:?}"
    );

    let child_keys: Vec<&str> = reported
        .lines()
        .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim()))
        .filter(|key| !key.is_empty() && !key.starts_with("PS") && !key.starts_with("__PS"))
        .collect();

    // `env_clear` has to keep the daemon's environment out. Rather than
    // mutating this process's environment (which needs `unsafe`, and this crate
    // denies it), the test compares the keys the target actually received
    // against the keys that exist in the ambient environment right now.
    let leaked: Vec<&str> = child_keys
        .iter()
        .copied()
        .filter(|key| std::env::var_os(key).is_some())
        .filter(|key| !BASELINE_KEYS.contains(key))
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
    let mut request = fixture.cmd_request(&format!("type nul > {}", cmd_path(&marker)));
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
    let mut required = fixture.cmd_request(&format!("type nul > {}", cmd_path(&marker)));
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

    let mut best_effort = fixture.cmd_request("exit 0");
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

/// Tree bugs are timing bugs, so a single green termination is not evidence.
#[tokio::test]
async fn repeated_timeout_termination_converges_every_time() {
    for iteration in 0..5 {
        let fixture = Fixture::new();
        let beat = fixture.path("leader.beat");
        let body = format!(
            "$f = {file}\n\
             while ($true) {{ Add-Content -Path $f -Value 'beat'; Start-Sleep -Milliseconds {beat} }}\n",
            file = ps_literal(&beat.to_string_lossy()),
            beat = BEAT_MILLIS,
        );
        let mut request = fixture.request(fixture.write_script("leader.ps1", &body));
        request.set_timeout(Duration::from_millis(1500));
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
