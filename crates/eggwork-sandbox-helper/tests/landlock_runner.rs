//! Integration coverage for the Landlock helper, driven through the real runner.
//!
//! The whole file is Linux-only, and the gate is at file level on purpose: every
//! test here needs the `landlock` kernel interface and a cgroup v2 hierarchy, so
//! the per-test `#[cfg(target_os = "linux")]` this replaces only left the
//! file's imports and helpers un-gated -- which is exactly what a Windows
//! `clippy -D warnings` build flagged (Foundation M004).
#![cfg(target_os = "linux")]

#[path = "support/helper_fixture.rs"]
mod helper_fixture;

use eggwork_core::{OverflowPolicy, Requirement};
use eggwork_runner::{
    ExecutionProvenance, ExecutionSetup, LocalProcessRunner, ResourceSetupRequest, RunnerError,
    RunnerRequest, SandboxOutcome, SandboxRequest, StdinPolicy, TrustedLandlockSetup,
};
use helper_fixture::TrustedHelperFixture;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The Cargo-built helper. Fixtures only ever *read* this path: it is build
/// output, not an installation directory, and the M004a invariant is that no
/// test mutates an executable another test may execute.
fn built_helper() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_eggwork-sandbox-helper"))
}

fn request(root: &Path, outside_read: &Path, outside_write: &Path) -> RunnerRequest {
    let mut request = RunnerRequest::new(vec![
            "/bin/sh".into(),
            "-c".into(),
            "cat input; printf ok > output; if cat \"$1\" >/dev/null 2>&1; then exit 77; fi; touch \"$2\" 2>/dev/null && exit 78; exit 0".into(),
            "landlock-test".into(),
            outside_read.display().to_string(),
            outside_write.display().to_string(),
        ], root);
    request.set_environment(vec![("PATH".into(), "/malicious/path".into())]);
    request.set_stdin_policy(StdinPolicy::Null);
    request.set_timeout(Duration::from_secs(5));
    request.set_output_policy(4096, 1024, OverflowPolicy::Truncate);
    request.set_provenance(ExecutionProvenance::default());
    request.set_sandbox_request(SandboxRequest::Required {
        profile: "workspace_rw".into(),
    });
    request.set_resource_setup_request(ResourceSetupRequest {
        memory_bytes: Requirement::NotRequested,
        cpu_millis: Requirement::NotRequested,
        pids: Requirement::NotRequested,
    });
    request
}

#[tokio::test]
async fn required_landlock_allows_workspace_and_denies_outside_reads_and_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("input"), b"inside\n").unwrap();
    let outside_read = temp.path().join("outside-secret");
    let outside_write = temp.path().join("outside-write");
    fs::write(&outside_read, b"must not be readable").unwrap();
    let fixture = TrustedHelperFixture::stage(built_helper());
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let (tx, _rx) = mpsc::channel(8);
    let result = runner
        .run(
            request(&root, &outside_read, &outside_write),
            CancellationToken::new(),
            tx,
        )
        .await
        .expect("required Landlock setup must be enforced");

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.head, b"inside\n");
    assert_eq!(fs::read(root.join("output")).unwrap(), b"ok");
    assert!(!outside_write.exists());
    assert!(matches!(
        result.setup.sandbox,
        eggwork_runner::SandboxOutcome::Applied { .. }
    ));
    assert!(matches!(
        result.execution_result().sandbox,
        Some(eggwork_core::SandboxResult::Applied { .. })
    ));
}

#[tokio::test]
async fn helper_trust_checks_reject_missing_wrong_and_symlinked_helpers() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().unwrap();
    // Every rejection case below is built from one fixture, so the negative
    // paths mutate only this test's private copy and can never disturb a
    // concurrently running test's helper.
    let fixture = TrustedHelperFixture::stage(built_helper());
    let helper = fixture.path().to_path_buf();
    let wrong = fixture.sibling("wrong");
    fs::write(&wrong, b"not an executable helper").unwrap();
    fs::set_permissions(&wrong, fs::Permissions::from_mode(0o644)).unwrap();
    let link = fixture.sibling("link");
    symlink(&helper, &link).unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let required_sandbox = SandboxRequest::Required {
        profile: "workspace_rw".into(),
    };
    let resources = ResourceSetupRequest {
        memory_bytes: Requirement::NotRequested,
        cpu_millis: Requirement::NotRequested,
        pids: Requirement::NotRequested,
    };

    for path in [temp.path().join("missing"), wrong, link] {
        let setup = TrustedLandlockSetup::new(path);
        assert!(
            setup
                .prepare(&required_sandbox, &resources, &root)
                .await
                .is_err()
        );
    }
    let best_effort = TrustedLandlockSetup::new(temp.path().join("missing"));
    let outcome = best_effort
        .prepare(
            &SandboxRequest::BestEffort {
                profile: "workspace_rw".into(),
            },
            &resources,
            &root,
        )
        .await
        .unwrap();
    assert!(matches!(outcome.sandbox, SandboxOutcome::NotApplied { .. }));

    let mut best_effort_request = request(&root, &root.join("unused"), &root.join("unused2"));
    best_effort_request.set_sandbox_request(SandboxRequest::BestEffort {
        profile: "workspace_rw".into(),
    });
    best_effort_request.set_argv(vec!["/bin/true".into()]);
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(temp.path().join("missing")));
    let (tx, _rx) = mpsc::channel(8);
    let result = runner
        .run(best_effort_request, CancellationToken::new(), tx)
        .await
        .expect("BestEffort should continue with an explicit unavailable outcome");
    assert_eq!(result.exit_code, Some(0));
    assert!(matches!(
        result.setup.sandbox,
        SandboxOutcome::NotApplied { .. }
    ));

    let marker = root.join("required-must-not-run");
    let mut required_request = request(&root, &root.join("unused"), &root.join("unused2"));
    required_request.set_argv(vec![
        "/usr/bin/touch".into(),
        marker.to_string_lossy().into_owned(),
    ]);
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(temp.path().join("missing")));
    let (tx, _rx) = mpsc::channel(8);
    assert!(
        runner
            .run(required_request, CancellationToken::new(), tx)
            .await
            .is_err()
    );
    assert!(!marker.exists());
}

#[tokio::test]
async fn workspace_symlink_cannot_read_outside_workspace() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let secret = temp.path().join("outside-secret");
    fs::write(&secret, b"outside\n").unwrap();
    symlink(&secret, root.join("escape")).unwrap();
    let fixture = TrustedHelperFixture::stage(built_helper());
    let mut request = request(&root, &secret, &temp.path().join("unused"));
    request.set_argv(vec!["/bin/cat".into(), "escape".into()]);
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let (tx, _rx) = mpsc::channel(8);
    let result = runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("required Landlock must be available on the test host");

    assert_ne!(result.exit_code, Some(0));
    assert!(result.stdout.head.is_empty());
    assert!(matches!(
        result.setup.sandbox,
        SandboxOutcome::Applied { .. }
    ));
}

#[tokio::test]
async fn cwd_symlink_outside_workspace_is_rejected_before_launch() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, root.join("escape")).unwrap();
    let mut request = request(&root, &outside.join("secret"), &outside.join("write"));
    request.set_working_directory(Some(eggwork_core::RelativePath::new("escape").unwrap()));
    let fixture = TrustedHelperFixture::stage(built_helper());
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let (tx, _rx) = mpsc::channel(8);
    let error = runner
        .run(request, CancellationToken::new(), tx)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        eggwork_runner::RunnerError::InvalidWorkingDirectory(_)
    ));
}

#[tokio::test]
async fn sandbox_timeout_and_cancellation_reap_the_helper_process_group() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let fixture = TrustedHelperFixture::stage(built_helper());
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let capabilities = runner.execution_capabilities().await;
    assert!(capabilities.contains(&"isolation.landlock.workspace-rw.v1".into()));
    assert!(capabilities.contains(&"resources.cgroups-v2.memory".into()));
    assert!(capabilities.contains(&"resources.cgroups-v2.cpu".into()));
    assert!(capabilities.contains(&"resources.cgroups-v2.pids".into()));
    let mut timed_request = request(&root, &root.join("unused"), &root.join("unused2"));
    timed_request.resource_setup_request_mut().cpu_millis = Requirement::Required(1000);
    timed_request.set_argv(vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()]);
    timed_request.set_timeout(Duration::from_millis(100));
    let (tx, _rx) = mpsc::channel(8);
    let timed = runner
        .run(timed_request, CancellationToken::new(), tx)
        .await
        .expect("sandboxed command should time out");
    assert_eq!(
        timed.termination,
        eggwork_runner::TerminationReason::TimedOut
    );

    let mut cancel_request = request(&root, &root.join("unused"), &root.join("unused2"));
    cancel_request.resource_setup_request_mut().cpu_millis = Requirement::Required(1000);
    cancel_request.set_argv(vec![
        "/bin/sh".into(),
        "-c".into(),
        "sleep 30 & echo $! > child-pid; wait".into(),
    ]);
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task_runner = runner;
    let task = tokio::spawn(async move {
        let (tx, _rx) = mpsc::channel(8);
        task_runner.run(cancel_request, task_cancellation, tx).await
    });
    for _ in 0..100 {
        if root.join("child-pid").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        root.join("child-pid").exists(),
        "target child did not start"
    );
    cancellation.cancel();
    let cancelled = task
        .await
        .unwrap()
        .expect("sandboxed cancellation should complete");
    assert_eq!(
        cancelled.termination,
        eggwork_runner::TerminationReason::Cancelled,
        "{cancelled:?}; stderr={}",
        String::from_utf8_lossy(&cancelled.stderr.head)
    );
    let child_pid = fs::read_to_string(root.join("child-pid"))
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let process_state = fs::read_to_string(format!("/proc/{child_pid}/stat"));
    assert!(
        process_state.is_err() || process_state.unwrap().split_whitespace().nth(2) == Some("Z")
    );
}

#[tokio::test]
async fn required_memory_limit_is_enforced_and_classified() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let fixture = TrustedHelperFixture::stage(built_helper());
    let mut request = request(&root, &root.join("unused"), &root.join("unused2"));
    request.set_sandbox_request(SandboxRequest::None);
    request.resource_setup_request_mut().memory_bytes = Requirement::Required(16 * 1024 * 1024);
    request.set_argv(vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        "x=bytearray(64*1024*1024); [x.__setitem__(i,1) for i in range(0,len(x),4096)]".into(),
    ]);
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let (tx, _rx) = mpsc::channel(8);
    let result = runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("required memory limit should run in its cgroup");
    let execution = result.execution_result();
    assert_eq!(
        execution.failure,
        Some(eggwork_core::ExecutionFailure::ResourceLimit)
    );
    assert!(matches!(
        execution.resources.unwrap().memory_bytes,
        eggwork_core::ResourceDimensionResult::LimitExceeded { .. }
    ));
}

#[tokio::test]
async fn required_pid_limit_is_enforced_and_classified() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let fixture = TrustedHelperFixture::stage(built_helper());
    let mut request = request(&root, &root.join("unused"), &root.join("unused2"));
    request.set_sandbox_request(SandboxRequest::None);
    request.resource_setup_request_mut().pids = Requirement::Required(12);
    request.set_argv(vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        "import os,time; kids=[]\nfor _ in range(64):\n try: pid=os.fork()\n except OSError: break\n if pid==0: time.sleep(0.2); os._exit(0)\n kids.append(pid)\nfor pid in kids: os.waitpid(pid,0)".into(),
    ]);
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let (tx, _rx) = mpsc::channel(8);
    let result = runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("required PID limit should run in its cgroup");
    let execution = result.execution_result();
    assert_eq!(
        execution.failure,
        Some(eggwork_core::ExecutionFailure::ResourceLimit)
    );
    assert!(matches!(
        execution.resources.unwrap().pids,
        eggwork_core::ResourceDimensionResult::LimitExceeded { .. }
    ));
}

#[tokio::test]
async fn required_cpu_quota_is_verified_before_target_start() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let fixture = TrustedHelperFixture::stage(built_helper());
    let mut request = request(&root, &root.join("unused"), &root.join("unused2"));
    request.set_sandbox_request(SandboxRequest::None);
    request.resource_setup_request_mut().cpu_millis = Requirement::Required(500);
    request.set_argv(vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf quota-ok".into(),
    ]);
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let (tx, _rx) = mpsc::channel(8);
    let result = runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("required CPU quota should be installed in the execution cgroup");
    assert_eq!(result.stdout.head, b"quota-ok");
    assert!(matches!(
        result.execution_result().resources.unwrap().cpu_millis,
        eggwork_core::ResourceDimensionResult::Applied { backend }
            if backend == "systemd-cgroup-v2"
    ));
}

#[tokio::test]
async fn concurrent_resource_scopes_keep_pid_limits_isolated() {
    let temp = tempfile::tempdir().unwrap();
    let low_root = temp.path().join("low");
    let high_root = temp.path().join("high");
    fs::create_dir(&low_root).unwrap();
    fs::create_dir(&high_root).unwrap();
    let fixture = TrustedHelperFixture::stage(built_helper());
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        "import os,time; kids=[]\nfor _ in range(8):\n try: pid=os.fork()\n except OSError: break\n if pid==0: time.sleep(0.2); os._exit(0)\n kids.append(pid)\nfor pid in kids: os.waitpid(pid,0)".into(),
    ];
    let mut low = request(
        &low_root,
        &low_root.join("unused"),
        &low_root.join("unused2"),
    );
    low.set_sandbox_request(SandboxRequest::None);
    low.resource_setup_request_mut().pids = Requirement::Required(6);
    low.set_argv(argv.clone());
    let mut high = request(
        &high_root,
        &high_root.join("unused"),
        &high_root.join("unused2"),
    );
    high.set_sandbox_request(SandboxRequest::None);
    high.resource_setup_request_mut().pids = Requirement::Required(32);
    high.set_argv(argv);
    let (low_tx, _low_rx) = mpsc::channel(8);
    let (high_tx, _high_rx) = mpsc::channel(8);
    let (low_result, high_result) = tokio::join!(
        runner.run(low, CancellationToken::new(), low_tx),
        runner.run(high, CancellationToken::new(), high_tx),
    );
    let low_resources = low_result.unwrap().execution_result().resources.unwrap();
    let high_resources = high_result.unwrap().execution_result().resources.unwrap();
    assert!(matches!(
        low_resources.pids,
        eggwork_core::ResourceDimensionResult::LimitExceeded { .. }
    ));
    assert!(matches!(
        high_resources.pids,
        eggwork_core::ResourceDimensionResult::Applied { .. }
    ));
}

#[test]
fn malformed_private_spec_and_unavailable_status_channel_do_not_launch_target() {
    use std::{io::Read, os::unix::fs::PermissionsExt};

    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let marker = temp.path().join("target-ran");
    let spec_path = temp.path().join("spec.json");
    fs::write(
        &spec_path,
        serde_json::json!({
            "schema_version": 1,
            "profile": "unsupported-profile",
            "root": temp.path(),
            "cwd": temp.path(),
            "argv": ["/usr/bin/touch", marker],
            "environment": []
        })
        .to_string(),
    )
    .unwrap();
    fs::set_permissions(&spec_path, fs::Permissions::from_mode(0o600)).unwrap();
    let status_path = temp.path().join("status.sock");
    let listener = std::os::unix::net::UnixListener::bind(&status_path).unwrap();
    let fixture = TrustedHelperFixture::stage(built_helper());
    let mut child = Command::new(fixture.path())
        .arg("--spec")
        .arg(&spec_path)
        .arg("--status")
        .arg(&status_path)
        .spawn()
        .unwrap();
    let (mut stream, _) = listener.accept().unwrap();
    let mut code = [0u8; 1];
    stream.read_exact(&mut code).unwrap();
    assert_eq!(code[0], 12);
    assert!(child.wait().unwrap().code().is_some_and(|code| code != 0));
    assert!(!marker.exists());

    let missing_socket_spec = temp.path().join("spec2.json");
    fs::write(
        &missing_socket_spec,
        serde_json::json!({
            "schema_version": 1,
            "profile": "workspace_rw",
            "root": temp.path(),
            "cwd": temp.path(),
            "argv": ["/usr/bin/touch", marker],
            "environment": []
        })
        .to_string(),
    )
    .unwrap();
    fs::set_permissions(&missing_socket_spec, fs::Permissions::from_mode(0o600)).unwrap();
    let mut child = Command::new(fixture.path())
        .arg("--spec")
        .arg(missing_socket_spec)
        .arg("--status")
        .arg(temp.path().join("missing.sock"))
        .spawn()
        .unwrap();
    assert!(child.wait().unwrap().code().is_some_and(|code| code != 0));
    assert!(!marker.exists());
}

// Operations M004a: the fixture-isolation proofs.
//
// The recorded CI failure was an `ETXTBSY` from a helper spawn, which is Linux
// refusing to open a running executable for writing. These tests assert the
// properties that make that whole class of failure structurally impossible in
// this suite, instead of adding a retry or a sleep and hoping.

#[tokio::test]
async fn two_fixtures_stage_distinct_executable_paths() {
    let first = TrustedHelperFixture::stage(built_helper());
    let second = TrustedHelperFixture::stage(built_helper());
    assert_ne!(
        first.path(),
        second.path(),
        "two fixtures must never name the same executable"
    );
    assert_ne!(first.directory(), second.directory());
    let built = helper_fixture::digest(built_helper());
    for fixture in [&first, &second] {
        assert!(fixture.path().exists(), "fixture must stage real bytes");
        assert_eq!(helper_fixture::digest(fixture.path()), built);
    }
}

#[tokio::test]
async fn staging_never_mutates_the_cargo_built_helper() {
    use std::os::unix::fs::PermissionsExt;

    let built = helper_fixture::digest(built_helper());
    let built_mode = fs::metadata(built_helper())
        .expect("build output metadata")
        .permissions()
        .mode()
        & 0o7777;
    for _ in 0..8 {
        let fixture = TrustedHelperFixture::stage(built_helper());
        assert_eq!(helper_fixture::digest(fixture.path()), built);
    }
    assert_eq!(
        fs::metadata(built_helper())
            .expect("build output metadata")
            .permissions()
            .mode()
            & 0o7777,
        built_mode,
        "staging must not change the build output's mode"
    );
    assert_eq!(
        helper_fixture::digest(built_helper()),
        built,
        "staging must treat build output as read-only input"
    );
}

/// A private workspace inside a fixture, writable only by its owner.
fn fixture_workspace(fixture: &TrustedHelperFixture) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let root = fixture.directory().join("workspace");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn required_sandbox_request(root: &Path, output: &str) -> RunnerRequest {
    let mut request = request(root, &root.join("unused"), &root.join("unused2"));
    request.set_sandbox_request(SandboxRequest::Required {
        profile: "workspace_rw".into(),
    });
    request.set_argv(vec![
        "/bin/sh".into(),
        "-c".into(),
        format!("printf {output}"),
    ]);
    request
}

#[tokio::test]
async fn concurrent_fixtures_both_execute_concurrently() {
    let first = TrustedHelperFixture::stage(built_helper());
    let second = TrustedHelperFixture::stage(built_helper());
    assert_ne!(first.path(), second.path());

    // Two fixtures, two runner instances, two workspaces, two executions in
    // flight at once: the shape that failed with `Text file busy` in CI.
    let first_root = fixture_workspace(&first);
    let second_root = fixture_workspace(&second);
    let first_runner = LocalProcessRunner::new(TrustedLandlockSetup::new(first.path()));
    let second_runner = LocalProcessRunner::new(TrustedLandlockSetup::new(second.path()));
    let (first_tx, _first_rx) = mpsc::channel(8);
    let (second_tx, _second_rx) = mpsc::channel(8);
    let (first_result, second_result) = tokio::join!(
        first_runner.run(
            required_sandbox_request(&first_root, "first-ok"),
            CancellationToken::new(),
            first_tx,
        ),
        second_runner.run(
            required_sandbox_request(&second_root, "second-ok"),
            CancellationToken::new(),
            second_tx,
        ),
    );
    let first_result = first_result.expect("first fixture executed");
    let second_result = second_result.expect("second fixture executed");
    assert_eq!(first_result.stdout.head, b"first-ok");
    assert_eq!(second_result.stdout.head, b"second-ok");
    // Non-vacuous: both fixtures really enforced isolation rather than
    // degrading to an unenforced run.
    assert!(matches!(
        first_result.execution_result().sandbox,
        Some(eggwork_core::SandboxResult::Applied { .. })
    ));
    assert!(matches!(
        second_result.execution_result().sandbox,
        Some(eggwork_core::SandboxResult::Applied { .. })
    ));
}

#[tokio::test]
async fn mutating_one_fixture_cannot_disturb_another() {
    let healthy = TrustedHelperFixture::stage(built_helper());
    let broken = TrustedHelperFixture::stage(built_helper());
    assert_ne!(healthy.path(), broken.path());
    let built = helper_fixture::digest(built_helper());

    // Break one fixture while the other is the one executing. Mode, bytes, and
    // path are all different files, so none of this can reach the healthy copy.
    broken.set_mode(0o644);
    let broken_capabilities = LocalProcessRunner::new(TrustedLandlockSetup::new(broken.path()))
        .execution_capabilities()
        .await;
    assert!(
        !broken_capabilities
            .iter()
            .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
        "a mode-broken fixture must stop advertising isolation; got {broken_capabilities:?}"
    );

    let root = fixture_workspace(&healthy);
    let (tx, _rx) = mpsc::channel(8);
    let result = LocalProcessRunner::new(TrustedLandlockSetup::new(healthy.path()))
        .run(
            required_sandbox_request(&root, "survivor-ok"),
            CancellationToken::new(),
            tx,
        )
        .await
        .expect("the untouched fixture still runs");
    assert_eq!(result.stdout.head, b"survivor-ok");
    assert!(matches!(
        result.execution_result().sandbox,
        Some(eggwork_core::SandboxResult::Applied { .. })
    ));
    assert_eq!(
        helper_fixture::digest(healthy.path()),
        built,
        "another fixture's mutations must not reach this copy"
    );

    // Replacing bytes is atomic and stays inside the broken fixture.
    //
    // `verify_trusted_helper` is a filesystem-trust check -- owner, mode, and
    // non-writable ancestors -- and deliberately does not identify the file, so
    // a correctly-permissioned junk file passes it. That is the intended
    // boundary: anything able to write inside an owner-only directory already
    // owns that installation. What must still hold is that such a fixture fails
    // loudly at execution time instead of reporting isolation as applied.
    broken.replace_with_junk(b"not an executable helper");
    assert_ne!(helper_fixture::digest(broken.path()), built);
    assert_eq!(helper_fixture::digest(healthy.path()), built);
    let junk_root = fixture_workspace(&broken);
    let (tx, _rx) = mpsc::channel(8);
    let junk_result = LocalProcessRunner::new(TrustedLandlockSetup::new(broken.path()))
        .run(
            required_sandbox_request(&junk_root, "never-printed"),
            CancellationToken::new(),
            tx,
        )
        .await;
    assert!(
        matches!(
            junk_result,
            Err(RunnerError::Spawn(_)) | Err(RunnerError::Setup(_))
        ),
        "a junk helper must fail loudly rather than claim isolation; got {junk_result:?}"
    );
    assert!(
        !broken
            .directory()
            .join("eggwork-sandbox-helper.staging")
            .exists(),
        "staging must not leave a temporary file behind"
    );
}

#[tokio::test]
async fn dropping_a_fixture_removes_only_its_own_directory() {
    let survivor = TrustedHelperFixture::stage(built_helper());
    let survivor_path = survivor.path().to_path_buf();
    let doomed_directory = {
        let doomed = TrustedHelperFixture::stage(built_helper());
        let directory = doomed.directory().to_path_buf();
        assert!(directory.exists());
        directory
        // `doomed` drops here. In every other test the fixture outlives
        // `run`, which returns only after the owned process tree has converged,
        // so cleanup never races an executing helper. Removing a directory does
        // not disturb a running process anyway -- its inode stays alive -- so an
        // early drop is untidy, never corrupting.
    };
    assert!(
        !doomed_directory.exists(),
        "a dropped fixture cleans up its own directory"
    );
    assert!(survivor_path.exists(), "drop is not a global cleanup");
}
