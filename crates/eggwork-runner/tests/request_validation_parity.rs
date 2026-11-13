//! The runner's request validation is a projection of the `eggwork_core`
//! domain contract, not a second copy of it.
//!
//! The defect this suite closes is a drift between two copies of one rule set.
//! The runner used to re-spell the environment-entry rules in a private helper,
//! and its copy was *weaker* than core's: it accepted a name containing `-` or
//! `.`, accepted a name beginning with a digit, and did not detect duplicate
//! names at all. The runner's public constructor and its public environment
//! setter reached that weaker gate without passing through core validation at
//! all, while the crate documentation claimed that callers could not bypass
//! protocol bounds.
//!
//! Every assertion here goes through the runner's **public** API. A test that
//! only exercised `eggwork_core` would have passed before the corrective and
//! would have proved nothing about the runner.
//!
//! The properties proved are:
//!
//! 1. Every public construction path reaches the same verdict as
//!    `from_spec`, for the same input, with the same typed error.
//! 2. The runner rejects every environment-name form core rejects.
//! 3. A duplicate environment name is refused by the duplicate-name rule, and
//!    the refusal is attributable to that rule specifically.
//! 4. The runner stays *stricter* than core on exactly one rule -- an empty
//!    argv entry -- and that rule is explicit.
//! 5. The stdin and output bounds are core's bounds.
//! 6. A request that violates the contract never reaches a child.

use eggwork_core::{
    CommandSpec, EnvironmentEntry, ExecutionSpec, IsolationRequirement, MAX_ARG_BYTES,
    MAX_ARG_COUNT, MAX_CAPTURE_BYTES, MAX_ENV_COUNT, MAX_ENV_NAME_BYTES, MAX_ENV_VALUE_BYTES,
    MAX_EVENT_CHUNK_BYTES, MAX_STDIN_BYTES, MAX_TIMEOUT, NetworkRequirement, OutputPolicy,
    OverflowPolicy, Requirement, ResourceRequirements, StdinPolicy, ValidationError,
};
use eggwork_runner::{
    ExecutionProvenance, LocalProcessRunner, NoExecutionSetup, ResourceSetupRequest, RunnerError,
    RunnerRequest,
};
use std::path::Path;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A valid command, used as the base for every case.
fn command() -> CommandSpec {
    CommandSpec {
        argv: vec!["/bin/echo".to_owned(), "hello".to_owned()],
        cwd: None,
        environment: Vec::new(),
        stdin: StdinPolicy::Null,
        timeout_millis: 5_000,
        output: OutputPolicy::default(),
        declared_outputs: Vec::new(),
        resources: ResourceRequirements {
            memory_bytes: Requirement::NotRequested,
            cpu_millis: Requirement::NotRequested,
            pids: Requirement::NotRequested,
        },
        isolation: IsolationRequirement::None,
        network: NetworkRequirement::Unrestricted,
    }
}

fn spec(command: CommandSpec) -> ExecutionSpec {
    ExecutionSpec {
        schema_version: 1,
        command,
        metadata: Vec::new(),
    }
}

/// Build the same request through the runner's *builder* path.
///
/// This is the path the corrective is about: `RunnerRequest::new` plus the
/// public setters, with no `eggwork_core` validation in between. It is the
/// only way an embedder can build a request without a protocol spec, so it is
/// the path that has to reach the same verdict.
fn build_through_the_builder(command: &CommandSpec, root: &Path) -> RunnerRequest {
    let mut request = RunnerRequest::new(command.argv.clone(), root);
    request.set_argv(command.argv.clone());
    request.set_working_directory(command.cwd.clone());
    request.set_environment(
        command
            .environment
            .iter()
            .map(|entry| (entry.name.clone(), entry.value.clone()))
            .collect(),
    );
    request.set_stdin_policy(command.stdin.clone());
    request.set_timeout(std::time::Duration::from_millis(command.timeout_millis));
    request.set_output_policy(
        command.output.capture_limit_bytes,
        command.output.event_chunk_bytes,
        command.output.overflow.clone(),
    );
    request.set_resource_setup_request(ResourceSetupRequest {
        memory_bytes: command.resources.memory_bytes,
        cpu_millis: command.resources.cpu_millis,
        pids: command.resources.pids,
    });
    request
}

/// Run a request and assert that it is refused before any child exists.
///
/// The marker file is the proof that the refusal happened *before* the spawn:
/// the target's only job would be to leave the marker behind.
async fn assert_refused_before_spawn(request: RunnerRequest, root: &Path) -> RunnerError {
    let marker = root.join("target-ran");
    let argv = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        format!("touch {}", marker.display()),
    ];
    assert_refused_before_spawn_with_argv(request, root, argv).await
}

/// The same, for a request whose argv is itself under test.
async fn assert_refused_before_spawn_with_argv(
    request: RunnerRequest,
    root: &Path,
    argv: Vec<String>,
) -> RunnerError {
    let marker = root.join("target-ran");
    let mut request = request;
    request.set_argv(argv);
    let (tx, _rx) = mpsc::channel(8);
    let error = LocalProcessRunner::new(NoExecutionSetup)
        .run(request, CancellationToken::new(), tx)
        .await
        .expect_err("the request must be refused");
    assert!(
        !marker.exists(),
        "a refused request must not reach a child: {error:?}"
    );
    error
}

fn environment(name: &str, value: &str) -> EnvironmentEntry {
    EnvironmentEntry {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

/// The names core refuses, and the reason each one is refused.
fn refused_environment_names() -> Vec<(&'static str, String)> {
    vec![
        ("a dash", "WITH-DASH".to_owned()),
        ("a dot", "WITH.DOT".to_owned()),
        ("a space", "WITH SPACE".to_owned()),
        ("an equals sign", "WITH=EQUALS".to_owned()),
        ("a leading digit", "1LEADING".to_owned()),
        ("a leading zero", "0LEADING".to_owned()),
        ("an empty name", String::new()),
        ("a NUL byte", "WITH\0NUL".to_owned()),
        (
            "a name one byte over the bound",
            "A".repeat(MAX_ENV_NAME_BYTES + 1),
        ),
    ]
}

/// The runner rejects every environment-name form core rejects.
///
/// This is the rule the runner's private copy used to get wrong, so the
/// assertion has to be made through the runner's public API and has to be
/// made about the *typed* error, not about a message.
#[tokio::test]
async fn the_runner_rejects_every_environment_name_form_core_rejects() {
    let root = tempfile::tempdir().expect("temp dir");

    for (label, name) in refused_environment_names() {
        // The builder path: the one an embedder uses.
        let mut request = RunnerRequest::new(command().argv.clone(), root.path());
        request.set_environment(vec![(name.clone(), "value".to_owned())]);
        assert!(
            matches!(
                request.contract_error(),
                Some(ValidationError::InvalidSyntax {
                    field: "environment name"
                })
            ),
            "{label}: the builder path must refuse {name:?} by the name rule; got {:?}",
            request.contract_error()
        );

        let error = assert_refused_before_spawn(request, root.path()).await;
        assert!(
            matches!(
                error,
                RunnerError::InvalidRequest(ValidationError::InvalidSyntax {
                    field: "environment name"
                })
            ),
            "{label}: the run must be refused by the name rule; got {error:?}"
        );

        // The protocol path: the same name, refused by core.
        let mut offending = command();
        offending.environment = vec![environment(&name, "value")];
        let error = RunnerRequest::from_spec(
            &spec(offending),
            root.path(),
            ExecutionProvenance::default(),
        )
        .expect_err("core must refuse the same name");
        assert!(
            matches!(
                error,
                RunnerError::InvalidRequest(ValidationError::InvalidSyntax {
                    field: "environment name"
                })
            ),
            "{label}: core must refuse {name:?} by the name rule; got {error:?}"
        );
    }
}

/// A name the contract accepts reaches the child.
#[tokio::test]
async fn a_legal_environment_name_is_accepted_by_the_runner() {
    let root = tempfile::tempdir().expect("temp dir");
    for name in ["NAME", "lower_case", "Mixed_Case_9", "_LEADING_UNDERSCORE"] {
        let mut request = RunnerRequest::new(command().argv.clone(), root.path());
        request.set_environment(vec![(name.to_owned(), "value".to_owned())]);
        assert!(
            request.contract_error().is_none(),
            "{name:?} is legal and must be accepted; got {:?}",
            request.contract_error()
        );
    }
}

/// A duplicate environment name is refused by the duplicate-name rule.
///
/// The attribution matters: a duplicate name is a *different* rule from a name
/// with invalid syntax, and the two must not collapse into one answer. Core
/// distinguishes them with distinct `field` labels, and the runner preserves
/// that distinction rather than flattening it into one message.
#[tokio::test]
async fn a_duplicate_environment_name_is_refused_by_the_duplicate_rule() {
    let root = tempfile::tempdir().expect("temp dir");

    let mut request = RunnerRequest::new(command().argv.clone(), root.path());
    request.set_environment(vec![
        ("DUPLICATE".to_owned(), "first".to_owned()),
        ("DUPLICATE".to_owned(), "second".to_owned()),
    ]);
    assert!(
        matches!(
            request.contract_error(),
            Some(ValidationError::InvalidSyntax {
                field: "duplicate environment name"
            })
        ),
        "a repeated name must be refused by the duplicate rule; got {:?}",
        request.contract_error()
    );

    let error = assert_refused_before_spawn(request, root.path()).await;
    assert!(
        matches!(
            error,
            RunnerError::InvalidRequest(ValidationError::InvalidSyntax {
                field: "duplicate environment name"
            })
        ),
        "the refusal must be attributable to the duplicate-name rule; got {error:?}"
    );

    // The same duplicate, through the protocol path, is refused by the same
    // rule -- so the runner is not applying a rule of its own here.
    let mut offending = command();
    offending.environment = vec![
        environment("DUPLICATE", "first"),
        environment("DUPLICATE", "second"),
    ];
    let error = RunnerRequest::from_spec(
        &spec(offending),
        root.path(),
        ExecutionProvenance::default(),
    )
    .expect_err("core must refuse the duplicate");
    assert!(
        matches!(
            error,
            RunnerError::InvalidRequest(ValidationError::InvalidSyntax {
                field: "duplicate environment name"
            })
        ),
        "core must refuse the duplicate by the duplicate rule; got {error:?}"
    );
}

/// The runner stays stricter than core on exactly one rule: an empty argv
/// entry.
///
/// `eggwork_core` permits an empty argv entry. The runner does not, because an
/// empty argument is a child-process construction hazard rather than a
/// protocol concern. The rule is the runner's own, it is documented in the
/// code, and it is the *only* place the runner may exceed its owner.
#[tokio::test]
async fn the_runner_refuses_an_empty_argv_entry_which_core_permits() {
    let root = tempfile::tempdir().expect("temp dir");

    let mut offending = command();
    offending.argv = vec!["/bin/echo".to_owned(), String::new()];

    // Core permits it.
    let accepted = RunnerRequest::from_spec(
        &spec(offending.clone()),
        root.path(),
        ExecutionProvenance::default(),
    );
    assert!(
        accepted.is_ok(),
        "core permits an empty argv entry; the runner's rule is stricter than core"
    );

    // The runner refuses it, and says which rule refused it.
    let request = build_through_the_builder(&offending, root.path());
    assert!(
        matches!(
            request.contract_error(),
            Some(ValidationError::InvalidSyntax {
                field: "argv entry"
            })
        ),
        "the runner must refuse an empty argv entry by its own rule; got {:?}",
        request.contract_error()
    );

    let error = assert_refused_before_spawn_with_argv(
        request,
        root.path(),
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!("touch {}", root.path().join("target-ran").display()),
            String::new(),
        ],
    )
    .await;
    assert!(
        matches!(
            error,
            RunnerError::InvalidRequest(ValidationError::InvalidSyntax {
                field: "argv entry"
            })
        ),
        "the refusal must name the argv-entry rule; got {error:?}"
    );
}

/// Every public construction path reaches the same verdict as `from_spec`.
///
/// This is the parity property the milestone is named for. For the same
/// input, the protocol path and the builder path must agree on the verdict
/// *and* on the typed error, so a caller cannot be told one thing by the
/// protocol and another by the builder.
#[tokio::test]
async fn every_public_construction_path_reaches_the_same_contract_verdict() {
    let root = tempfile::tempdir().expect("temp dir");

    let mut cases: Vec<(&'static str, CommandSpec)> = Vec::new();

    cases.push(("a valid command", command()));

    let mut empty_argv = command();
    empty_argv.argv = Vec::new();
    cases.push(("an empty argv", empty_argv));

    let mut long_argv_entry = command();
    long_argv_entry.argv = vec!["A".repeat(MAX_ARG_BYTES + 1)];
    cases.push(("an oversized argv entry", long_argv_entry));

    let mut many_args = command();
    many_args.argv = (0..=MAX_ARG_COUNT)
        .map(|index| format!("arg{index}"))
        .collect();
    cases.push(("too many argv entries", many_args));

    let mut bad_env_name = command();
    bad_env_name.environment = vec![environment("WITH-DASH", "value")];
    cases.push(("an environment name with a dash", bad_env_name));

    let mut duplicate_env = command();
    duplicate_env.environment = vec![
        environment("DUPLICATE", "first"),
        environment("DUPLICATE", "second"),
    ];
    cases.push(("a duplicate environment name", duplicate_env));

    let mut long_env_value = command();
    long_env_value.environment = vec![EnvironmentEntry {
        name: "NAME".to_owned(),
        value: "v".repeat(MAX_ENV_VALUE_BYTES + 1),
    }];
    cases.push(("an oversized environment value", long_env_value));

    let mut nul_env_value = command();
    nul_env_value.environment = vec![environment("NAME", "value\0with-nul")];
    cases.push(("a NUL in an environment value", nul_env_value));

    let mut too_many_env = command();
    too_many_env.environment = (0..=MAX_ENV_COUNT)
        .map(|index| environment(&format!("NAME_{index}"), "value"))
        .collect();
    cases.push(("too many environment entries", too_many_env));

    let mut zero_timeout = command();
    zero_timeout.timeout_millis = 0;
    cases.push(("a zero timeout", zero_timeout));

    let mut long_timeout = command();
    long_timeout.timeout_millis = MAX_TIMEOUT.as_millis() as u64 + 1;
    cases.push(("an over-long timeout", long_timeout));

    let mut big_stdin = command();
    big_stdin.stdin = StdinPolicy::Bytes(vec![0; MAX_STDIN_BYTES + 1]);
    cases.push(("oversized stdin", big_stdin));

    let mut big_capture = command();
    big_capture.output.capture_limit_bytes = MAX_CAPTURE_BYTES + 1;
    cases.push(("an over-large capture limit", big_capture));

    let mut zero_chunk = command();
    zero_chunk.output.event_chunk_bytes = 0;
    cases.push(("a zero event chunk", zero_chunk));

    let mut big_chunk = command();
    big_chunk.output.event_chunk_bytes = MAX_EVENT_CHUNK_BYTES + 1;
    cases.push(("an over-large event chunk", big_chunk));

    for (label, offending) in cases {
        let via_protocol = RunnerRequest::from_spec(
            &spec(offending.clone()),
            root.path(),
            ExecutionProvenance::default(),
        );
        let via_builder = build_through_the_builder(&offending, root.path());
        let builder_verdict = via_builder.contract_error();

        match via_protocol {
            Ok(_) => assert!(
                builder_verdict.is_none(),
                "{label}: the protocol path accepts, so the builder path must accept too; \
                 the builder reported {builder_verdict:?}"
            ),
            Err(RunnerError::InvalidRequest(expected)) => assert_eq!(
                builder_verdict.as_ref(),
                Some(&expected),
                "{label}: the two construction paths must report the same typed error"
            ),
            Err(other) => panic!("{label}: unexpected error {other:?}"),
        }
    }
}

/// The mutable resource door is a construction path too, and it is covered.
///
/// `resource_setup_request_mut` hands out a mutable reference, so it cannot
/// validate on the way in. The enforcement point has to catch what it let
/// through, which is exactly the case that would otherwise be a bypass.
#[tokio::test]
async fn the_mutable_resource_door_is_covered_by_the_contract() {
    let root = tempfile::tempdir().expect("temp dir");

    let mut request = RunnerRequest::new(command().argv.clone(), root.path());
    request.resource_setup_request_mut().memory_bytes = Requirement::Required(u64::MAX);
    assert!(
        matches!(
            request.contract_error(),
            Some(ValidationError::OutOfRange {
                field: "resource requirements"
            })
        ),
        "an out-of-range resource requirement must be caught; got {:?}",
        request.contract_error()
    );

    let error = assert_refused_before_spawn(request, root.path()).await;
    assert!(
        matches!(
            error,
            RunnerError::InvalidRequest(ValidationError::OutOfRange {
                field: "resource requirements"
            })
        ),
        "the run must be refused by the resource rule; got {error:?}"
    );
}

/// The stdin bound is core's bound, and the runner still forwards the bytes.
///
/// The runner's local `StdinPolicy` duplicate is gone: the public path
/// `eggwork_runner::StdinPolicy` now names `eggwork_core::StdinPolicy`. The
/// assignment below is a compile-time proof of that, so the test cannot pass
/// against a runner that kept a second type.
#[tokio::test]
async fn the_runner_forwards_stdin_bytes_and_enforces_the_core_stdin_bound() {
    let core_policy: StdinPolicy = eggwork_runner::StdinPolicy::Null;
    assert!(matches!(core_policy, StdinPolicy::Null));

    let root = tempfile::tempdir().expect("temp dir");

    // The bound is core's bound, enforced through the runner's public API.
    let mut request = RunnerRequest::new(command().argv.clone(), root.path());
    request.set_stdin_policy(eggwork_runner::StdinPolicy::Bytes(vec![
        0;
        MAX_STDIN_BYTES + 1
    ]));
    assert!(
        matches!(
            request.contract_error(),
            Some(ValidationError::TooLong {
                field: "stdin",
                max: MAX_STDIN_BYTES
            })
        ),
        "stdin over the core bound must be refused; got {:?}",
        request.contract_error()
    );
    let error = assert_refused_before_spawn(request, root.path()).await;
    assert!(
        matches!(
            error,
            RunnerError::InvalidRequest(ValidationError::TooLong {
                field: "stdin",
                max: MAX_STDIN_BYTES
            })
        ),
        "the refusal must name the stdin bound; got {error:?}"
    );

    // A stdin at the bound is accepted.
    let mut request = RunnerRequest::new(command().argv.clone(), root.path());
    request.set_stdin_policy(eggwork_runner::StdinPolicy::Bytes(vec![0; MAX_STDIN_BYTES]));
    assert!(
        request.contract_error().is_none(),
        "stdin exactly at the core bound must be accepted; got {:?}",
        request.contract_error()
    );
}

/// The bytes still reach the child.
#[cfg(unix)]
#[tokio::test]
async fn stdin_bytes_are_forwarded_to_the_child() {
    let root = tempfile::tempdir().expect("temp dir");
    let mut request = RunnerRequest::new(vec!["/bin/cat".to_owned()], root.path());
    request.set_stdin_policy(eggwork_runner::StdinPolicy::Bytes(
        b"forwarded-stdin\n".to_vec(),
    ));
    let (tx, mut rx) = mpsc::channel(8);
    let result = LocalProcessRunner::new(NoExecutionSetup)
        .run(request, CancellationToken::new(), tx)
        .await
        .expect("the target runs");
    let mut captured = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        if let eggwork_runner::OutputChunk::Stdout(bytes) = chunk {
            captured.extend_from_slice(&bytes);
        }
    }
    assert_eq!(captured, b"forwarded-stdin\n");
    assert_eq!(result.exit_code, Some(0));
}

/// The output bounds are core's bounds.
#[tokio::test]
async fn the_runner_enforces_the_core_output_bounds() {
    let root = tempfile::tempdir().expect("temp dir");

    let cases: Vec<(&'static str, usize, usize)> = vec![
        ("an over-large capture limit", MAX_CAPTURE_BYTES + 1, 4096),
        ("a zero event chunk", 1024, 0),
        ("an over-large event chunk", 1024, MAX_EVENT_CHUNK_BYTES + 1),
    ];

    for (label, capture_limit, event_chunk_bytes) in cases {
        let mut request = RunnerRequest::new(command().argv.clone(), root.path());
        request.set_output_policy(capture_limit, event_chunk_bytes, OverflowPolicy::Truncate);
        assert!(
            matches!(
                request.contract_error(),
                Some(ValidationError::OutOfRange {
                    field: "output policy"
                })
            ),
            "{label}: must be refused by the core output rule; got {:?}",
            request.contract_error()
        );
        let error = assert_refused_before_spawn(request, root.path()).await;
        assert!(
            matches!(
                error,
                RunnerError::InvalidRequest(ValidationError::OutOfRange {
                    field: "output policy"
                })
            ),
            "{label}: the refusal must name the output policy; got {error:?}"
        );
    }

    // A policy exactly at the bounds is accepted.
    let mut request = RunnerRequest::new(command().argv.clone(), root.path());
    request.set_output_policy(
        MAX_CAPTURE_BYTES,
        MAX_EVENT_CHUNK_BYTES,
        OverflowPolicy::Terminate,
    );
    assert!(
        request.contract_error().is_none(),
        "a policy at the core bounds must be accepted; got {:?}",
        request.contract_error()
    );
}

/// The runner's public request surface is pinned.
///
/// The builder path is the runner's embedding surface, and a new public entry
/// point on `RunnerRequest` is a new way to produce a request. This test
/// fails when one is added, so the new entry point has to be reviewed against
/// the core contract rather than slipped in.
#[test]
fn the_runner_request_public_surface_is_pinned() {
    let source = include_str!("../src/lib.rs");
    let start = source
        .find("impl RunnerRequest {")
        .expect("the RunnerRequest impl block exists");
    let body = &source[start..];
    let mut depth = 0usize;
    let mut end = body.len();
    for (index, character) in body.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = index;
                    break;
                }
            }
            _ => {}
        }
    }
    let mut found: Vec<&str> = body[..end]
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub fn "))
        .filter_map(|rest| rest.split('(').next())
        .map(|name| name.trim())
        .collect();
    found.sort_unstable();

    let mut expected = vec![
        "new",
        "from_spec",
        "contract_error",
        "set_argv",
        "set_working_directory",
        "set_environment",
        "set_stdin_policy",
        "set_timeout",
        "set_output_policy",
        "set_provenance",
        "set_sandbox_request",
        "set_resource_setup_request",
        "resource_setup_request_mut",
        "sandbox_request",
        "resource_setup_request",
    ];
    expected.sort_unstable();

    assert_eq!(
        found, expected,
        "the public surface of RunnerRequest changed: a new public entry point is a new \
         way to produce a request and has to be reviewed against the core contract"
    );
}
