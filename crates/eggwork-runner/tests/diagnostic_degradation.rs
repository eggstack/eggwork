//! A degraded best-effort execution has to say so *while it runs*, not only in
//! the terminal result.
//!
//! The runner is the only party that knows why an enforcement was not applied,
//! and the output channel is the only carrier that preserves ordering between
//! the runner's knowledge and the target's output. These tests pin that
//! ordering at the layer where it is decided: the diagnostic chunk has to be
//! sent before the target produces a single byte, and it has to be sent at all
//! exactly when something was degraded.
//!
//! `NoExecutionSetup` is used explicitly rather than `LocalProcessRunner::default`:
//! the default picks a setup from the environment, so a test that wanted "no
//! enforcement available" could silently get an enforcing one.
//!
//! The argv is platform specific because the runner has platform-specific spawn
//! paths and the diagnostic sits in front of all of them: the Windows CI job
//! runs this suite, and `/bin/sh` does not exist there.

use eggwork_core::Requirement;
use eggwork_runner::{
    LocalProcessRunner, NoExecutionSetup, OutputChunk, ResourceSetupRequest, RunnerError,
    RunnerRequest, SandboxRequest,
};
use std::path::Path;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A command that writes one line to stdout.
fn echo_argv(text: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![comspec(), "/C".to_owned(), format!("echo {text}")]
    } else {
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!("echo {text}"),
        ]
    }
}

/// A command that creates a marker file and nothing else.
fn touch_argv(marker: &Path) -> Vec<String> {
    if cfg!(windows) {
        vec![
            comspec(),
            "/C".to_owned(),
            format!("type nul > \"{}\"", marker.display()),
        ]
    } else {
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!("touch {}", marker.display()),
        ]
    }
}

/// The command interpreter Windows actually uses.
///
/// It is resolved from the environment rather than hard-coded, because the
/// Windows path spawns through a Job Object and the target has to be a real
/// Windows program.
fn comspec() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
}

/// Collects every chunk the runner sent, in the order it sent them.
async fn drain(mut rx: mpsc::Receiver<OutputChunk>) -> Vec<OutputChunk> {
    let mut chunks = Vec::new();
    while let Some(chunk) = rx.recv().await {
        chunks.push(chunk);
    }
    chunks
}

/// The diagnostics among the collected chunks, in order.
fn diagnostics(chunks: &[OutputChunk]) -> Vec<&str> {
    chunks
        .iter()
        .filter_map(|chunk| match chunk {
            OutputChunk::Diagnostic(payload) => Some(payload.as_str()),
            _ => None,
        })
        .collect()
}

/// The index of the first target output chunk.
fn first_target_output_index(chunks: &[OutputChunk]) -> Option<usize> {
    chunks
        .iter()
        .position(|chunk| matches!(chunk, OutputChunk::Stdout(_) | OutputChunk::Stderr(_)))
}

/// A best-effort sandbox request that cannot be honoured emits exactly one
/// diagnostic, and it is emitted before the target writes anything.
#[tokio::test]
async fn a_degraded_best_effort_sandbox_is_reported_before_the_target_output() {
    let root = tempfile::tempdir().expect("temp dir");
    let runner = LocalProcessRunner::new(NoExecutionSetup);
    let mut request = RunnerRequest::new(echo_argv("target-output"), root.path());
    request.set_sandbox_request(SandboxRequest::BestEffort {
        profile: "workspace_rw".into(),
    });

    let (tx, rx) = mpsc::channel(32);
    let result = runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("the target itself runs");
    let chunks = drain(rx).await;

    let reported = diagnostics(&chunks);
    assert_eq!(
        reported.len(),
        1,
        "one degraded dimension produces exactly one diagnostic; got {reported:?}"
    );
    let payload = reported[0];
    assert!(
        payload.starts_with("eggwork.diagnostic.isolation.degraded.v1 "),
        "unexpected diagnostic payload: {payload}"
    );
    assert!(payload.contains(" requirement=best_effort"));
    assert!(payload.contains(" profile=workspace_rw"));
    assert!(payload.contains(" reason="));
    assert!(
        eggwork_core::DiagnosticPayload::parse(payload).is_ok(),
        "the payload must match the documented grammar: {payload}"
    );

    // The ordering guarantee: the diagnostic precedes every target output.
    let first_output = first_target_output_index(&chunks).expect("the target produced output");
    assert_eq!(
        first_output, 1,
        "the diagnostic has to precede the target's first output; got {chunks:?}"
    );
    assert!(
        matches!(chunks.first(), Some(OutputChunk::Diagnostic(_))),
        "the diagnostic has to be the first chunk on the channel; got {chunks:?}"
    );

    // And the terminal result still records the degradation, so the stream and
    // the snapshot cannot disagree.
    let execution = result.execution_result();
    assert!(matches!(
        execution.sandbox,
        Some(eggwork_core::SandboxResult::NotApplied { .. })
    ));
}

/// A resource degradation names the affected dimension.
#[tokio::test]
async fn a_degraded_best_effort_resource_request_is_reported_per_dimension() {
    let root = tempfile::tempdir().expect("temp dir");
    let runner = LocalProcessRunner::new(NoExecutionSetup);
    let mut request = RunnerRequest::new(echo_argv("done"), root.path());
    request.set_resource_setup_request(ResourceSetupRequest {
        memory_bytes: Requirement::BestEffort(64 * 1024 * 1024),
        cpu_millis: Requirement::NotRequested,
        pids: Requirement::NotRequested,
    });

    let (tx, rx) = mpsc::channel(32);
    runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("the target itself runs");
    let chunks = drain(rx).await;

    let reported = diagnostics(&chunks);
    assert_eq!(
        reported.len(),
        1,
        "only the requested dimension is affected; got {reported:?}"
    );
    assert!(reported[0].starts_with("eggwork.diagnostic.resources.degraded.v1 "));
    assert!(reported[0].contains(" dimension=memory"));
    assert!(reported[0].contains(" requirement=best_effort"));
}

/// Two affected dimensions produce two diagnostics, one per dimension.
#[tokio::test]
async fn two_affected_resource_dimensions_produce_one_diagnostic_each() {
    let root = tempfile::tempdir().expect("temp dir");
    let runner = LocalProcessRunner::new(NoExecutionSetup);
    let mut request = RunnerRequest::new(echo_argv("done"), root.path());
    request.set_resource_setup_request(ResourceSetupRequest {
        memory_bytes: Requirement::BestEffort(64 * 1024 * 1024),
        cpu_millis: Requirement::BestEffort(1000),
        pids: Requirement::NotRequested,
    });

    let (tx, rx) = mpsc::channel(32);
    runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("the target itself runs");
    let chunks = drain(rx).await;

    let reported = diagnostics(&chunks);
    assert_eq!(reported.len(), 2, "got {reported:?}");
    assert!(
        reported
            .iter()
            .any(|payload| payload.contains(" dimension=memory"))
    );
    assert!(
        reported
            .iter()
            .any(|payload| payload.contains(" dimension=cpu"))
    );
    assert!(
        reported
            .iter()
            .all(|payload| !payload.contains(" dimension=pids")),
        "a dimension that was not requested is not affected; got {reported:?}"
    );
}

/// A required request is refused, not degraded: the target never runs and no
/// diagnostic is emitted.
#[tokio::test]
async fn a_required_sandbox_is_refused_without_a_degradation_diagnostic() {
    let root = tempfile::tempdir().expect("temp dir");
    let marker = root.path().join("marker");
    let runner = LocalProcessRunner::new(NoExecutionSetup);
    let mut request = RunnerRequest::new(touch_argv(&marker), root.path());
    request.set_sandbox_request(SandboxRequest::Required {
        profile: "workspace_rw".into(),
    });

    let (tx, rx) = mpsc::channel(32);
    let error = runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect_err("a required sandbox must fail closed");
    assert!(
        matches!(error, RunnerError::RequiredSetupUnavailable),
        "unexpected error: {error:?}"
    );
    assert!(
        !marker.exists(),
        "the target must not have run for a refused required request"
    );
    let chunks = drain(rx).await;
    assert!(
        diagnostics(&chunks).is_empty(),
        "a refusal is not a degradation; got {:?}",
        diagnostics(&chunks)
    );
}

/// Nothing degraded means nothing reported.
#[tokio::test]
async fn an_execution_with_nothing_degraded_emits_no_diagnostic() {
    let root = tempfile::tempdir().expect("temp dir");
    let runner = LocalProcessRunner::new(NoExecutionSetup);
    let request = RunnerRequest::new(echo_argv("quiet"), root.path());

    let (tx, rx) = mpsc::channel(32);
    runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("the target itself runs");
    let chunks = drain(rx).await;
    assert!(
        diagnostics(&chunks).is_empty(),
        "an execution with no degradation must emit no diagnostic; got {:?}",
        diagnostics(&chunks)
    );
}

/// A cancellation before the spawn is not a degradation either.
#[tokio::test]
async fn an_execution_cancelled_before_spawn_emits_no_diagnostic() {
    let root = tempfile::tempdir().expect("temp dir");
    let runner = LocalProcessRunner::new(NoExecutionSetup);
    let mut request = RunnerRequest::new(echo_argv("never"), root.path());
    request.set_sandbox_request(SandboxRequest::BestEffort {
        profile: "workspace_rw".into(),
    });

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let (tx, rx) = mpsc::channel(32);
    let error = runner
        .run(request, cancellation, tx)
        .await
        .expect_err("a cancelled execution does not run");
    assert!(
        matches!(error, RunnerError::CancelledBeforeSpawn),
        "unexpected error: {error:?}"
    );
    let chunks = drain(rx).await;
    assert!(
        diagnostics(&chunks).is_empty(),
        "a cancellation is not a degradation; got {:?}",
        diagnostics(&chunks)
    );
}

/// The diagnostic payload stays inside the bound the grammar documents.
#[tokio::test]
async fn a_diagnostic_payload_is_bounded() {
    let root = tempfile::tempdir().expect("temp dir");
    let runner = LocalProcessRunner::new(NoExecutionSetup);
    let mut request = RunnerRequest::new(echo_argv("bounded"), root.path());
    request.set_sandbox_request(SandboxRequest::BestEffort {
        profile: "workspace_rw".into(),
    });

    let (tx, rx) = mpsc::channel(32);
    runner
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("the target itself runs");
    let chunks = drain(rx).await;
    for payload in diagnostics(&chunks) {
        assert!(
            payload.len() <= eggwork_core::MAX_DIAGNOSTIC_PAYLOAD_BYTES,
            "the diagnostic payload must stay within its bound; got {} bytes",
            payload.len()
        );
    }
}
