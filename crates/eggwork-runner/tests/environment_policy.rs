//! The runner's environment construction, observed through its public API.
//!
//! The runner builds a child's environment in one place, and both of its
//! launch paths — the direct path and the sandbox-helper path — use it. The
//! rules it applies come from the shared environment policy, whose single
//! owner is `crates/eggwork-runner/policy/environment-policy.toml` and whose
//! generated copy this crate compiles.
//!
//! The live defect these tests close was that the caller's `LANG` was
//! *appended* to the node's baseline. The child saw the caller's locale, the
//! launch specification carried two `LANG` entries, the helper refused the
//! specification, and — for a best-effort request only — the runner answered
//! that pre-target refusal by re-spawning the target directly with no
//! Landlock ruleset at all. The observable result was a successful, entirely
//! unconfined execution whose only record of the degradation was a `NotApplied`
//! in the terminal snapshot.
//!
//! The helper-side half of that proof — that the helper *accepts* the
//! resulting specification and that the execution keeps its confinement — is
//! in `crates/eggwork-sandbox-helper/tests/environment_policy_parity.rs`,
//! because it needs the real helper executable.

use eggwork_core::{OverflowPolicy, Requirement};
use eggwork_runner::{
    LocalProcessRunner, NoExecutionSetup, ResourceSetupRequest, RunnerRequest, SandboxRequest,
    StdinPolicy, environment_policy,
};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The environment the child actually received, sorted.
async fn observed_environment(root: &Path, environment: Vec<(String, String)>) -> Vec<String> {
    let mut request = RunnerRequest::new(vec!["/bin/sh".into(), "-c".into(), "env".into()], root);
    request.set_environment(environment);
    request.set_stdin_policy(StdinPolicy::Null);
    request.set_timeout(Duration::from_secs(20));
    request.set_output_policy(16384, 4096, OverflowPolicy::Truncate);
    request.set_sandbox_request(SandboxRequest::BestEffort {
        profile: "workspace_rw".into(),
    });
    request.set_resource_setup_request(ResourceSetupRequest {
        memory_bytes: Requirement::NotRequested,
        cpu_millis: Requirement::NotRequested,
        pids: Requirement::NotRequested,
    });
    let (tx, _rx) = mpsc::channel(8);
    let result = LocalProcessRunner::new(NoExecutionSetup)
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("the execution runs");
    let mut lines: Vec<String> = String::from_utf8_lossy(&result.stdout.head)
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

/// The policy module is the shared contract, and it is reachable from the
/// runner's public API.
///
/// The standalone sandbox helper enforces the same policy and may not depend
/// on any workspace crate, so it compiles a generated copy of the same source.
/// The two copies are pinned together by a parity test; this test is what
/// makes the runner's copy reachable from outside the crate, which is what
/// that parity test compares against.
#[test]
fn the_policy_is_reachable_from_the_runner_public_api() {
    assert!(
        !environment_policy::DENIED_KEYS.is_empty(),
        "the denied-key list must not be empty"
    );
    assert_eq!(
        environment_policy::MATCH_RULE,
        "ascii-uppercase-exact-or-prefix",
        "the matching rule is part of the contract, not an implementation detail"
    );
    assert_eq!(
        environment_policy::PATH_VALUE,
        "/usr/bin:/bin",
        "the `PATH` contract is a Linux/Unix contract with one value"
    );
    for entry in ["LD_", "PATH", "IFS", "GIT_CONFIG_", "AWS_", "SSH_AUTH_SOCK"] {
        assert!(
            environment_policy::DENIED_KEYS.contains(&entry),
            "{entry} is part of the reviewed deny-list and must not have been dropped"
        );
    }
}

/// The node-owned class and the node's own baseline are the same set.
///
/// If they could differ, the runner would discard a caller's value for a key
/// the helper still expected to carry the node's value, and the helper would
/// refuse the launch.
#[test]
fn the_node_owned_class_is_the_node_owned_baseline() {
    let baseline_keys: Vec<&str> = environment_policy::BASELINE
        .iter()
        .map(|(key, _)| *key)
        .collect();
    assert_eq!(
        environment_policy::RESERVED_KEYS,
        baseline_keys.as_slice(),
        "the node-owned class and the node-owned baseline must be the same list"
    );
}

/// The baseline the runner installs is the policy's baseline.
///
/// On Unix the two are the same list; the Windows baseline is runner-local,
/// and this suite runs on Unix.
#[cfg(unix)]
#[test]
fn the_installed_baseline_is_the_policy_baseline() {
    let installed = eggwork_runner::baseline_environment_keys();
    let policy: Vec<&str> = environment_policy::BASELINE
        .iter()
        .map(|(key, _)| *key)
        .collect();
    assert_eq!(installed, policy);
}

/// A caller's value for a node-owned key does not reach the child.
///
/// This is the regression for the live defect, observed on the direct path.
#[cfg(unix)]
#[tokio::test]
async fn a_caller_supplied_node_owned_key_does_not_reach_the_child() {
    let root = tempfile::tempdir().expect("temp dir");
    let observed = observed_environment(
        root.path(),
        vec![
            ("LANG".into(), "en_US.UTF-8".into()),
            ("LC_ALL".into(), "en_US.UTF-8".into()),
            ("CI".into(), "false".into()),
            ("TERM".into(), "xterm".into()),
            ("PAGER".into(), "less".into()),
        ],
    )
    .await;
    for key in environment_policy::RESERVED_KEYS {
        let expected = environment_policy::baseline_value(key).expect("baseline value");
        assert!(
            observed.contains(&format!("{key}={expected}")),
            "the child must carry the node's {key}: {observed:?}"
        );
    }
    assert!(
        !observed.iter().any(|entry| entry.starts_with("LANG=en_US")),
        "the caller's LANG must not reach the child: {observed:?}"
    );
    assert_eq!(
        observed
            .iter()
            .filter(|entry| entry.starts_with("LANG="))
            .count(),
        1,
        "the child must see exactly one LANG: {observed:?}"
    );
}

/// A denied key does not reach the child.
///
/// The runner drops it silently: it is the request-normalisation boundary. The
/// helper's refusal of the same key is a different layer and a different
/// behaviour, and only the list is shared between them.
#[cfg(unix)]
#[tokio::test]
async fn a_caller_supplied_denied_key_does_not_reach_the_child() {
    let root = tempfile::tempdir().expect("temp dir");
    let observed = observed_environment(
        root.path(),
        vec![
            ("LD_PRELOAD".into(), "/tmp/evil.so".into()),
            ("AWS_SECRET_ACCESS_KEY".into(), "secret".into()),
            ("PYTHONPATH".into(), "/tmp".into()),
            ("CALLER_KEY".into(), "kept".into()),
        ],
    )
    .await;
    assert!(
        !observed
            .iter()
            .any(|entry| entry.starts_with("LD_PRELOAD=")),
        "a denied key must not reach the child: {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|entry| entry.starts_with("AWS_SECRET_ACCESS_KEY=")),
        "a denied key must not reach the child: {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|entry| entry.starts_with("PYTHONPATH=")),
        "a denied key must not reach the child: {observed:?}"
    );
    assert!(
        observed.iter().any(|entry| entry == "CALLER_KEY=kept"),
        "a caller-controlled key must reach the child: {observed:?}"
    );
}
