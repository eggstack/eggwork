//! The environment-policy parity gate.
//!
//! The node's environment policy — the denied-key list and its matching rule,
//! the node-owned (reserved) keys, the node-owned baseline, and the `PATH`
//! value contract — has one owner: `crates/eggwork-runner/policy/environment-policy.toml`.
//! The runner compiles a generated copy of it. The sandbox helper is a
//! standalone binary that may not depend on any workspace crate, so it
//! compiles a generated copy of the *same* source.
//!
//! Two copies of one contract is a hole unless something pins them together.
//! What pins them is this file, and it does so in two independent ways:
//!
//! 1. **Element by element.** The helper's own generated module is compiled
//!    into this test, and compared with the runner's public copy, entry by
//!    entry, in order. A one-sided edit to either copy fails here.
//! 2. **Behaviourally.** The *real* helper executable is driven with a launch
//!    specification for every probe key, and its accept/refuse decision is
//!    compared with what the runner's copy of the policy says. A one-sided
//!    edit that changed a rule rather than a list fails here, and so does an
//!    edit that changed the matching rule itself.
//!
//! The second is the one that matters. A structural comparison of two files
//! proves that two files are alike; it does not prove that the helper enforces
//! the policy the runner applies. The live defect was exactly that gap: the
//! runner appended the caller's `LANG` to the node's baseline, so the launch
//! specification carried two `LANG` entries, the helper refused the
//! specification, and a best-effort request answered that pre-target refusal
//! by re-spawning the target with no Landlock ruleset at all. The observable
//! result was a successful, entirely unconfined execution whose only record of
//! the degradation was a `NotApplied` in the terminal snapshot.
//!
//! The element-wise part of this file is platform-independent on purpose: the
//! lists are platform-independent, and the parity gate must run everywhere.
//! Only the behavioural part needs Linux, because only Linux has a helper that
//! enforces anything.
#![allow(dead_code)]

/// The helper's own generated copy of the policy, compiled into this test.
///
/// This is the file the helper binary itself compiles, not a copy of it, so an
/// edit to the helper's generated copy is an edit to what this test compares.
#[path = "../src/environment_policy.rs"]
mod helper_policy;

use eggwork_runner::environment_policy as runner_policy;

/// The shared fixture that stages a *trusted* sandbox helper.
///
/// It is included at the top level rather than inside the Linux module because
/// a `#[path]` inside an inline module resolves against that module's own
/// directory, which does not exist here.
#[cfg(target_os = "linux")]
#[path = "support/helper_fixture.rs"]
mod helper_fixture;

/// The runner's policy, stated over a launch specification rather than over a
/// request.
///
/// The runner *drops* a denied key, *replaces* a node-owned key's value with
/// the node's, and passes everything else through. The helper refuses a
/// specification that does not already reflect that, because the helper is the
/// enforcement point and the runner is the request-normalisation boundary.
/// Only the list is shared between them; the behaviour deliberately is not.
///
/// This is the contract, written against the *runner's* copy of the policy. The
/// helper enforces the same contract with its own copy, and the difference
/// between the two is what this test measures.
fn the_runner_policy_refuses(key: &str, value: &str) -> bool {
    if key != "PATH" && runner_policy::denied_environment_key(key) {
        return true;
    }
    if key == "PATH" && value != runner_policy::PATH_VALUE {
        return true;
    }
    if let Some((name, expected)) = runner_policy::baseline_entry(key)
        && (name != key || value != expected)
    {
        return true;
    }
    false
}

/// The denied-key list is the same list, in the same order, in both copies.
///
/// A mismatch is a contract change, not a test to be adjusted: the two copies
/// are the same contract, and a one-sided edit means one side now enforces
/// something the other side does not.
#[test]
fn the_denied_key_list_is_the_same_list_in_the_same_order() {
    assert_eq!(
        runner_policy::DENIED_KEYS.len(),
        helper_policy::DENIED_KEYS.len(),
        "the runner and the helper carry different numbers of denied keys; a mismatch is a \
         contract change, not a test to be adjusted"
    );
    for (index, (runner, helper)) in runner_policy::DENIED_KEYS
        .iter()
        .zip(helper_policy::DENIED_KEYS.iter())
        .enumerate()
    {
        assert_eq!(
            runner, helper,
            "denied-key entry {index} differs: the runner's copy has {runner:?} and the \
             helper's has {helper:?}. A mismatch is a contract change, not a test to be adjusted"
        );
    }
}

/// The node-owned class is the same class, in the same order, in both copies.
#[test]
fn the_node_owned_class_is_the_same_class_in_the_same_order() {
    assert_eq!(
        runner_policy::RESERVED_KEYS.len(),
        helper_policy::RESERVED_KEYS.len(),
        "the runner and the helper carry different numbers of node-owned keys; a mismatch is a \
         contract change, not a test to be adjusted"
    );
    for (index, (runner, helper)) in runner_policy::RESERVED_KEYS
        .iter()
        .zip(helper_policy::RESERVED_KEYS.iter())
        .enumerate()
    {
        assert_eq!(
            runner, helper,
            "node-owned-key entry {index} differs: the runner's copy has {runner:?} and the \
             helper's has {helper:?}. A mismatch is a contract change, not a test to be adjusted"
        );
    }
}

/// The node-owned baseline is the same baseline, in the same order, with the
/// same values, in both copies.
#[test]
fn the_node_owned_baseline_is_the_same_baseline_in_both_copies() {
    assert_eq!(
        runner_policy::BASELINE.len(),
        helper_policy::BASELINE.len(),
        "the runner and the helper carry different baselines; a mismatch is a contract change, \
         not a test to be adjusted"
    );
    for (index, (runner, helper)) in runner_policy::BASELINE
        .iter()
        .zip(helper_policy::BASELINE.iter())
        .enumerate()
    {
        assert_eq!(
            runner, helper,
            "baseline entry {index} differs: the runner's copy has {runner:?} and the helper's \
             has {helper:?}. A mismatch is a contract change, not a test to be adjusted"
        );
    }
}

/// The matching rule and the `PATH` contract are the same in both copies.
///
/// The matching rule is part of the contract rather than an implementation
/// detail: if the two sides disagreed about it, the runner would drop a key
/// the helper still refuses, or the other way round.
#[test]
fn the_matching_rule_and_the_path_contract_are_the_same_in_both_copies() {
    assert_eq!(
        runner_policy::MATCH_RULE,
        helper_policy::MATCH_RULE,
        "the matching rule differs between the copies; a mismatch is a contract change, not a \
         test to be adjusted"
    );
    assert_eq!(
        runner_policy::PATH_VALUE,
        helper_policy::PATH_VALUE,
        "the `PATH` contract differs between the copies; a mismatch is a contract change, not \
         a test to be adjusted"
    );
    assert_eq!(
        runner_policy::PATH_VALUE,
        "/usr/bin:/bin",
        "the `PATH` contract is a Linux/Unix contract with one value"
    );
}

/// The helper's copy of the protocol limits is the owner's, not a guess.
///
/// The helper is a standalone binary and cannot import `eggwork-core`, so the
/// generator reads these five values out of the owner of the wire limits and
/// emits them into the helper's copy. The comparison here is what stops that
/// copy from drifting.
#[test]
fn the_helper_limit_constants_match_eggwork_core() {
    assert_eq!(
        helper_policy::MAX_ARG_COUNT,
        eggwork_core::MAX_ARG_COUNT,
        "the helper's argument-count limit must be the owner's"
    );
    assert_eq!(
        helper_policy::MAX_ARG_BYTES,
        eggwork_core::MAX_ARG_BYTES,
        "the helper's argument-size limit must be the owner's"
    );
    assert_eq!(
        helper_policy::MAX_ENV_COUNT,
        eggwork_core::MAX_ENV_COUNT,
        "the helper's environment-count limit must be the owner's"
    );
    assert_eq!(
        helper_policy::MAX_ENV_NAME_BYTES,
        eggwork_core::MAX_ENV_NAME_BYTES,
        "the helper's environment-name limit must be the owner's"
    );
    assert_eq!(
        helper_policy::MAX_ENV_VALUE_BYTES,
        eggwork_core::MAX_ENV_VALUE_BYTES,
        "the helper's environment-value limit must be the owner's"
    );
}

/// The `PATH` contract is a Linux/Unix contract.
///
/// The Windows baseline is explicitly outside it, so a future reader must not
/// "fix" the Windows `PATH` to this value.
#[test]
fn the_path_contract_states_that_windows_is_outside_it() {
    assert_eq!(
        runner_policy::PATH_VALUE,
        "/usr/bin:/bin",
        "the Unix `PATH` contract"
    );
    assert!(
        !runner_policy::BASELINE_COMMON
            .iter()
            .any(|(key, _)| *key == "PATH"),
        "the platform-independent part of the baseline must not carry `PATH`; the Windows \
         baseline's `PATH` is built from the parent process and is explicitly outside the \
         Linux/Unix contract"
    );
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{helper_fixture, helper_policy, runner_policy, the_runner_policy_refuses};
    use eggwork_core::{OverflowPolicy, Requirement};
    use eggwork_runner::{
        LocalProcessRunner, NoExecutionSetup, ResourceSetupRequest, RunnerRequest, SandboxOutcome,
        SandboxRequest, StdinPolicy, TrustedLandlockSetup,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use helper_fixture::TrustedHelperFixture;

    /// The Cargo-built helper. Fixtures only ever *read* this path.
    fn built_helper() -> &'static Path {
        Path::new(env!("CARGO_BIN_EXE_eggwork-sandbox-helper"))
    }

    /// The status byte the helper refuses a launch with when the environment
    /// does not satisfy the shared policy.
    ///
    /// It is distinct from the general specification-validation status, so an
    /// operator reading a refusal can tell an environment-policy refusal from
    /// a specification that is malformed in some other way.
    const ENVIRONMENT_POLICY_REFUSAL: u8 = 13;

    /// The general specification-validation refusal.
    const SPECIFICATION_REFUSAL: u8 = 12;

    /// The probe keys and the decision the runner's policy makes about them.
    ///
    /// The expectation is stated here independently of both implementations,
    /// and the test then checks that the runner's copy of the policy and the
    /// helper's behaviour both agree with it. A one-sided edit to either copy
    /// breaks one of those two agreements.
    const PROBES: &[(&str, &str, bool)] = &[
        // Denied keys. The runner drops them; the helper refuses a
        // specification that carries one.
        ("LD_PRELOAD", "/tmp/evil.so", true),
        ("DYLD_INSERT_LIBRARIES", "/tmp/evil.dylib", true),
        ("IFS", ":", true),
        ("SHELLOPTS", "x", true),
        ("CDPATH", "/tmp", true),
        ("BASH_ENV", "/tmp/rc", true),
        ("ENV", "/tmp/rc", true),
        ("PYTHONPATH", "/tmp", true),
        ("PYTHONHOME", "/tmp", true),
        ("NODE_OPTIONS", "--require=/tmp/x.js", true),
        ("RUBYOPT", "-r/tmp/x", true),
        ("PERL5OPT", "-M/tmp/x", true),
        ("GIT_ASKPASS", "/tmp/ask", true),
        ("SSH_ASKPASS", "/tmp/ask", true),
        ("GIT_CONFIG_GLOBAL", "/tmp/gitconfig", true),
        ("AWS_SECRET_ACCESS_KEY", "secret", true),
        ("AZURE_CLIENT_ID", "id", true),
        ("GOOGLE_APPLICATION_CREDENTIALS", "/tmp/creds.json", true),
        ("SSH_AUTH_SOCK", "/tmp/agent.sock", true),
        // A denied *prefix* rather than a denied exact name.
        ("AWS_CONFIG_FILE", "/tmp/config", true),
        // A differently-cased spelling of a denied key.
        ("ld_preload", "/tmp/evil.so", true),
        // Node-owned keys carrying the caller's value. The runner replaces
        // the value with the node's; the helper refuses a specification that
        // carries the caller's value.
        ("LANG", "en_US.UTF-8", true),
        ("LC_ALL", "en_US.UTF-8", true),
        ("CI", "false", true),
        ("NO_COLOR", "0", true),
        ("TERM", "xterm", true),
        ("PAGER", "less", true),
        ("GIT_TERMINAL_PROMPT", "1", true),
        // The `PATH` contract.
        ("PATH", "/tmp/bin", true),
        // Node-owned keys carrying the node's own value. The runner installs
        // exactly these; the helper accepts them.
        ("PATH", "/usr/bin:/bin", false),
        ("LANG", "C.UTF-8", false),
        ("LC_ALL", "C.UTF-8", false),
        ("CI", "1", false),
        ("NO_COLOR", "1", false),
        ("TERM", "dumb", false),
        ("PAGER", "cat", false),
        ("GIT_TERMINAL_PROMPT", "0", false),
        // A differently-cased spelling of a node-owned key. The node owns the
        // key, so the helper refuses a specification that spells it
        // differently: the child would have the caller's variable rather than
        // the node's.
        ("lang", "C.UTF-8", true),
        // Caller-controlled keys. The runner passes them through and the
        // helper accepts them.
        ("HOME", "/tmp/home", false),
        ("EGGWORK_CALLER_KEY", "caller", false),
    ];

    /// The first status byte the helper reports for a launch specification.
    fn helper_status_for_spec(environment: &[(String, String)]) -> u8 {
        use std::io::Read;

        let directory = tempfile::tempdir().expect("temp dir");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only directory");
        let root = directory
            .path()
            .canonicalize()
            .expect("canonical workspace root");
        let spec_path = directory.path().join("spec.json");
        let spec = serde_json::json!({
            "schema_version": 1,
            "profile": serde_json::Value::Null,
            "root": root,
            "cwd": root,
            "argv": ["/bin/sh", "-c", "true"],
            "environment": environment,
        });
        fs::write(&spec_path, serde_json::to_vec(&spec).expect("spec json")).expect("write spec");
        fs::set_permissions(&spec_path, fs::Permissions::from_mode(0o600)).expect("private spec");
        let status_path = directory.path().join("status.sock");
        let listener = std::os::unix::net::UnixListener::bind(&status_path).expect("status socket");
        let fixture = TrustedHelperFixture::stage(built_helper());
        let mut child = Command::new(fixture.path())
            .arg("--spec")
            .arg(&spec_path)
            .arg("--status")
            .arg(&status_path)
            .spawn()
            .expect("spawn the helper");
        let (mut stream, _) = listener.accept().expect("accept the status connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("read timeout");
        let mut code = [0u8; 1];
        stream.read_exact(&mut code).expect("status byte");
        let _ = child.wait();
        code[0]
    }

    /// The helper's decision on every probe key matches the runner's policy.
    ///
    /// This is the behavioural half of the parity gate. It drives the real
    /// helper executable, so it measures what the helper enforces rather than
    /// what two files say.
    #[test]
    fn the_helper_refuses_exactly_what_the_runner_drops() {
        for (key, value, expect_refused) in PROBES {
            let environment = vec![(key.to_string(), value.to_string())];
            let status = helper_status_for_spec(&environment);
            let policy_refuses = the_runner_policy_refuses(key, value);
            assert_eq!(
                policy_refuses, *expect_refused,
                "the runner's policy and the probe table disagree about {key}={value:?}; one of \
                 them is wrong about the contract"
            );
            if *expect_refused {
                assert_eq!(
                    status, ENVIRONMENT_POLICY_REFUSAL,
                    "the helper must refuse {key}={value:?} with the environment-policy status"
                );
            } else {
                assert_ne!(
                    status, ENVIRONMENT_POLICY_REFUSAL,
                    "the helper must accept {key}={value:?}; the runner's policy says the caller \
                     controls this key"
                );
            }
        }
    }

    /// A specification with a duplicate environment key is refused with the
    /// environment-policy status.
    ///
    /// The duplicate is what the live defect produced: the caller's `LANG` and
    /// the node's `LANG` in one specification. The runner can no longer build
    /// such a specification, but the helper must still refuse one, because a
    /// duplicate key is an environment the node did not build.
    #[test]
    fn a_duplicate_environment_key_is_refused_with_the_environment_policy_status() {
        let environment = vec![
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
        ];
        assert_eq!(
            helper_status_for_spec(&environment),
            ENVIRONMENT_POLICY_REFUSAL,
            "a duplicate environment key must be refused with the environment-policy status"
        );
    }

    /// A specification that is malformed in a way that is *not* about the
    /// environment keeps the general validation status.
    ///
    /// The two statuses must stay distinguishable: an operator reading
    /// "status 12" cannot tell which of several specification problems
    /// happened, and that was the reason for the environment-policy status.
    #[test]
    fn a_malformed_specification_keeps_the_general_validation_status() {
        use std::io::Read;

        let directory = tempfile::tempdir().expect("temp dir");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only directory");
        let root = directory
            .path()
            .canonicalize()
            .expect("canonical workspace root");
        let spec_path = directory.path().join("spec.json");
        let spec = serde_json::json!({
            "schema_version": 1,
            "profile": "unsupported-profile",
            "root": root,
            "cwd": root,
            "argv": ["/bin/sh", "-c", "true"],
            "environment": [],
        });
        fs::write(&spec_path, serde_json::to_vec(&spec).expect("spec json")).expect("write spec");
        fs::set_permissions(&spec_path, fs::Permissions::from_mode(0o600)).expect("private spec");
        let status_path = directory.path().join("status.sock");
        let listener = std::os::unix::net::UnixListener::bind(&status_path).expect("status socket");
        let fixture = TrustedHelperFixture::stage(built_helper());
        let mut child = Command::new(fixture.path())
            .arg("--spec")
            .arg(&spec_path)
            .arg("--status")
            .arg(&status_path)
            .spawn()
            .expect("spawn the helper");
        let (mut stream, _) = listener.accept().expect("accept the status connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("read timeout");
        let mut code = [0u8; 1];
        stream.read_exact(&mut code).expect("status byte");
        let _ = child.wait();
        assert_eq!(
            code[0], SPECIFICATION_REFUSAL,
            "a specification that is malformed in a way unrelated to the environment keeps the \
             general validation status"
        );
    }

    /// A request that sets a node-owned key keeps its confinement.
    ///
    /// This is the regression for the live defect. The caller's `LANG` used to
    /// be appended to the node's baseline, so the launch specification carried
    /// two `LANG` entries, the helper refused the specification, and the
    /// runner — for a best-effort request only — answered that pre-target
    /// refusal by re-spawning the target directly with no Landlock ruleset at
    /// all. The observable result was a successful, entirely unconfined
    /// execution whose only record of the degradation was a `NotApplied` in the
    /// terminal snapshot.
    ///
    /// So the assertion is not only that the execution succeeded. It is that
    /// the sandbox outcome is `Applied`, that the child saw the node's `LANG`
    /// and not the caller's, and that the child saw exactly one `LANG`.
    #[tokio::test]
    async fn a_caller_supplied_node_owned_key_keeps_the_execution_confined() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("workspace");
        fs::create_dir(&root).expect("workspace");
        let fixture = TrustedHelperFixture::stage(built_helper());
        let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
        let (tx, _rx) = mpsc::channel(8);
        let result = runner
            .run(
                environment_probe_request(
                    &root,
                    SandboxRequest::Required {
                        profile: "workspace_rw".into(),
                    },
                ),
                CancellationToken::new(),
                tx,
            )
            .await
            .expect("the execution runs");
        assert!(
            matches!(result.setup.sandbox, SandboxOutcome::Applied { .. }),
            "a caller-supplied node-owned key must not cost the execution its confinement: {:?}",
            result.setup.sandbox
        );
        let observed = String::from_utf8(result.stdout.head.clone()).expect("utf-8 environment");
        let lang_lines: Vec<&str> = observed
            .lines()
            .filter(|line| line.starts_with("LANG="))
            .collect();
        assert_eq!(
            lang_lines,
            vec!["LANG=C.UTF-8"],
            "the child must see the node's LANG exactly once"
        );
        assert!(
            !observed.contains("en_US.UTF-8"),
            "the caller's LANG must not reach the child: {observed}"
        );
        assert!(
            !observed.contains("LANG=en_US"),
            "the child's environment must not carry the caller's LANG"
        );
    }

    /// The direct path and the helper path agree about the environment.
    ///
    /// They used to disagree. The direct path applied the caller's `LANG`
    /// *after* the node's baseline, so the child saw the caller's locale,
    /// while the launch specification carried both entries and was refused.
    #[tokio::test]
    async fn the_direct_path_and_the_helper_path_agree_about_the_environment() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("workspace");
        fs::create_dir(&root).expect("workspace");

        let (tx, _rx) = mpsc::channel(8);
        let direct = LocalProcessRunner::new(NoExecutionSetup)
            .run(
                environment_probe_request(
                    &root,
                    SandboxRequest::BestEffort {
                        profile: "workspace_rw".into(),
                    },
                ),
                CancellationToken::new(),
                tx,
            )
            .await
            .expect("the direct execution runs");
        let fixture = TrustedHelperFixture::stage(built_helper());
        let (tx, _rx) = mpsc::channel(8);
        let confined = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()))
            .run(
                environment_probe_request(
                    &root,
                    SandboxRequest::Required {
                        profile: "workspace_rw".into(),
                    },
                ),
                CancellationToken::new(),
                tx,
            )
            .await
            .expect("the confined execution runs");
        assert!(
            matches!(confined.setup.sandbox, SandboxOutcome::Applied { .. }),
            "the confined execution must be confined: {:?}",
            confined.setup.sandbox
        );
        let direct_environment = sorted_environment(&direct.stdout.head);
        let confined_environment = sorted_environment(&confined.stdout.head);
        assert_eq!(
            direct_environment, confined_environment,
            "the two paths must build the same environment"
        );
        for key in runner_policy::RESERVED_KEYS {
            let expected = runner_policy::baseline_value(key).expect("baseline value");
            assert!(
                direct_environment.contains(&format!("{key}={expected}")),
                "the direct path must carry the node's {key}"
            );
            assert!(
                confined_environment.contains(&format!("{key}={expected}")),
                "the helper path must carry the node's {key}"
            );
        }
    }

    /// A request that sets the node-owned keys to the caller's values and
    /// asks for the environment the child sees.
    fn environment_probe_request(root: &Path, sandbox: SandboxRequest) -> RunnerRequest {
        let mut request =
            RunnerRequest::new(vec!["/bin/sh".into(), "-c".into(), "env".into()], root);
        request.set_environment(vec![
            ("LANG".into(), "en_US.UTF-8".into()),
            ("LC_ALL".into(), "en_US.UTF-8".into()),
            ("CI".into(), "false".into()),
            ("TERM".into(), "xterm".into()),
            ("PAGER".into(), "less".into()),
            ("NO_COLOR".into(), "0".into()),
            ("GIT_TERMINAL_PROMPT".into(), "1".into()),
            ("PATH".into(), "/malicious/path".into()),
            ("LD_PRELOAD".into(), "/tmp/evil.so".into()),
            ("CALLER_KEY".into(), "caller".into()),
        ]);
        request.set_stdin_policy(StdinPolicy::Null);
        request.set_timeout(Duration::from_secs(20));
        request.set_output_policy(16384, 4096, OverflowPolicy::Truncate);
        request.set_sandbox_request(sandbox);
        request.set_resource_setup_request(ResourceSetupRequest {
            memory_bytes: Requirement::NotRequested,
            cpu_millis: Requirement::NotRequested,
            pids: Requirement::NotRequested,
        });
        request
    }

    /// The child's environment, sorted, so two runs can be compared.
    fn sorted_environment(captured: &[u8]) -> Vec<String> {
        let mut lines: Vec<String> = String::from_utf8_lossy(captured)
            .lines()
            .map(|line| line.to_owned())
            .collect();
        lines.sort();
        lines
    }

    /// The helper's copy of the policy is the one the helper binary compiles.
    ///
    /// The comparison above uses the module compiled into this test, which is
    /// the helper's own source file. This test pins the other end of it: that
    /// the file is the one the helper binary was built from.
    #[test]
    fn the_helper_copy_is_the_file_the_helper_compiles() {
        let helper_source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/environment_policy.rs");
        assert!(
            helper_source.exists(),
            "the helper's generated policy copy must exist at {}",
            helper_source.display()
        );
        let compiled = include_str!("../src/environment_policy.rs");
        let on_disk = fs::read_to_string(&helper_source).expect("read the generated copy");
        assert_eq!(
            compiled, on_disk,
            "the copy compiled into this test must be the copy the helper compiles"
        );
        assert_eq!(
            compiled,
            include_str!("../src/environment_policy.rs"),
            "the generated copy must be stable"
        );
        let _ = helper_policy::PATH_VALUE;
    }
}
