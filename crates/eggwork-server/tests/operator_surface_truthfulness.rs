//! The operator surface says what it means.
//!
//! Two defects are proved against here:
//!
//! * the metric vocabulary had no owner that was the producer, so five live
//!   counters were incremented by the server and dropped by the report
//!   (`operator_metric_coverage.rs` carries the before/after evidence for
//!   that one, and `operator_execution_counts.rs` the naming evidence);
//! * every configuration fault collapsed into `configuration is invalid or
//!   unreadable`, so an operator with one wrong file mode was told nothing
//!   about which rule failed.
//!
//! The tests drive the real operator binary wherever the behaviour under test
//! lives in the binary, because the binary is the operator surface.

mod common;

use common::{config_json, configuration_detail, eggworkd, install, installed, set_mode};
use eggwork_server::operations::{ConfigRule, OperationsError, OperatorConfig};
use std::path::Path;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// OP-1 — the metric vocabulary has a producer owner
// ---------------------------------------------------------------------------

/// The reported counter set is exactly the producers' vocabulary.
///
/// The vocabulary is declared once, with the producers, and the report path
/// reads that declaration. This is the set-equality check: a metric the
/// server can increment but the report does not carry fails here.
#[test]
fn the_reported_metric_set_is_exactly_the_producers_vocabulary() {
    let temp = TempDir::new().unwrap();
    let config_path = install(temp.path(), "127.0.0.1:0");
    let config = OperatorConfig::load(&config_path).unwrap();
    let report = eggwork_server::operations::metrics_snapshot(&config).unwrap();
    let mut reported: Vec<&str> = report.counters.keys().map(String::as_str).collect();
    reported.sort();
    let mut declared: Vec<&str> = eggwork_server::Metric::ALL
        .iter()
        .map(|metric| metric.name())
        .collect();
    declared.sort();
    assert_eq!(
        reported, declared,
        "the reported counter set and the producers' vocabulary must be the same set"
    );
}

/// No increment site names a metric outside the vocabulary.
///
/// The vocabulary is a typed enumeration, so an increment site that named a
/// metric by string would be the one way to reintroduce the drift: the
/// counter would be incremented and never reported. The scan is a source
/// check because "a second way to name a metric" cannot be observed from
/// outside the crate.
#[test]
fn no_increment_site_names_a_metric_outside_the_vocabulary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the server crate lives inside the workspace");
    for relative in [
        "crates/eggwork-server/src/lib.rs",
        "crates/eggwork-server/src/operations.rs",
    ] {
        let source = std::fs::read_to_string(root.join(relative)).unwrap();
        let production = match source.find("#[cfg(test)]") {
            Some(index) => &source[..index],
            None => source.as_str(),
        };
        for (number, line) in production.lines().enumerate() {
            let calls = line.contains("increment_metric(") || line.contains("increment_metrics(");
            if !calls {
                continue;
            }
            assert!(
                !line.contains('"'),
                "{relative}:{} names a metric as a string; the vocabulary owns the names",
                number + 1
            );
        }
    }
}

// ---------------------------------------------------------------------------
// OP-3 — actionable configuration failures
// ---------------------------------------------------------------------------

/// Every rule has a stable, greppable identifier and an action.
#[test]
fn every_configuration_rule_has_a_stable_identifier_and_an_action() {
    let mut identifiers: Vec<&str> = Vec::new();
    for rule in ConfigRule::ALL {
        let id = rule.id();
        assert!(!id.is_empty(), "a rule must have an identifier");
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "the rule identifier {id} must be greppable"
        );
        assert!(!identifiers.contains(&id), "{id} is declared twice");
        identifiers.push(id);
        let action = rule.action();
        assert!(!action.is_empty(), "{id} must say what to do");
        assert!(
            action.split_whitespace().count() >= 4,
            "the action for {id} must be a sentence, not a token: {action}"
        );
    }
}

/// The rules an unprivileged test cannot construct are still declared.
///
/// A file owned by somebody else cannot be created without the privilege to
/// change a file's owner, so the wrong-owner family is asserted through the
/// vocabulary rather than through a fault. The same is true of the rules that
/// name a failure the current shapes cannot reach.
#[test]
fn the_rules_an_unprivileged_test_cannot_construct_are_still_declared() {
    for rule in [
        ConfigRule::ConfigFileWrongOwner,
        ConfigRule::TlsCertificateWrongOwner,
        ConfigRule::TlsPrivateKeyWrongOwner,
        ConfigRule::StateRootWrongOwner,
        ConfigRule::DatabaseDirectoryWrongOwner,
        ConfigRule::DatabaseFileWrongOwner,
        ConfigRule::ConfigSerializationFailed,
        ConfigRule::SandboxHelperUnsupportedPlatform,
    ] {
        assert!(
            !rule.id().is_empty() && !rule.action().is_empty(),
            "{rule:?} must carry an identifier and an action"
        );
    }
}

/// The configuration fault a rule reports, or a panic naming what was
/// reported instead.
fn fault_of(error: OperationsError) -> eggwork_server::operations::ConfigFault {
    match error {
        OperationsError::Config(fault) => fault,
        other => panic!("expected a configuration fault, got {other:?}"),
    }
}

/// One test per rule, asserting the reported identifier and the message.
macro_rules! configuration_rule_tests {
    ($($(#[$meta:meta])* $name:ident => $rule:ident, $build:expr, $contains:expr;)*) => {
        $(
            $(#[$meta])*
            #[test]
            fn $name() {
                let temp = TempDir::new().unwrap();
                let error = ($build)(temp.path());
                let fault = fault_of(error);
                assert_eq!(
                    fault.rule,
                    ConfigRule::$rule,
                    "the wrong rule was reported: {fault}"
                );
                let message = fault.to_string();
                assert!(
                    message.starts_with(&format!("[{}] ", fault.rule.id())),
                    "the message must lead with the rule identifier: {message}"
                );
                assert!(
                    message.contains($contains),
                    "the message must name the offending item: {message}"
                );
                assert!(
                    message.ends_with(fault.rule.action()),
                    "the message must end with the corrective action: {message}"
                );
            }
        )*
    };
}

/// Load a configuration file that is not there.
fn load_failure(root: &Path) -> OperationsError {
    OperatorConfig::load(&root.join("absent.json")).unwrap_err()
}

/// Validate a configuration document.
fn validate_failure(value: serde_json::Value) -> OperationsError {
    serde_json::from_value::<OperatorConfig>(value)
        .expect("the document is a configuration")
        .validate()
        .unwrap_err()
}

configuration_rule_tests! {
    a_missing_configuration_file_is_reported_as_unreadable => ConfigFileUnreadable,
        load_failure, "absent.json";
    a_configuration_that_is_not_a_regular_file_is_reported => ConfigFileNotARegularFile,
        |root: &Path| {
            let directory = root.join("a-directory");
            std::fs::create_dir_all(&directory).unwrap();
            OperatorConfig::load(&directory).unwrap_err()
        },
        "a-directory";
    an_oversized_configuration_file_is_reported => ConfigFileTooLarge,
        |root: &Path| {
            let path = root.join("big.json");
            std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
            set_mode(&path, 0o644);
            OperatorConfig::load(&path).unwrap_err()
        },
        "big.json";
    a_world_writable_configuration_file_is_reported => ConfigFileWorldWritable,
        |root: &Path| {
            let path = root.join("loose.json");
            std::fs::write(&path, b"{}").unwrap();
            set_mode(&path, 0o666);
            OperatorConfig::load(&path).unwrap_err()
        },
        "loose.json";
    a_malformed_configuration_file_is_reported => ConfigFileMalformed,
        |root: &Path| {
            let path = root.join("broken.json");
            std::fs::write(&path, b"{not json").unwrap();
            set_mode(&path, 0o644);
            OperatorConfig::load(&path).unwrap_err()
        },
        "broken.json";
    an_unknown_configuration_field_is_reported => ConfigFieldInvalid,
        |root: &Path| {
            let path = root.join("extra.json");
            std::fs::write(&path, br#"{"schema_version":1,"unexpected":true}"#).unwrap();
            set_mode(&path, 0o644);
            OperatorConfig::load(&path).unwrap_err()
        },
        "extra.json";

    // The configuration's own fields.
    an_unsupported_schema_version_is_reported => ConfigSchemaVersionUnsupported,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["schema_version"] = serde_json::json!(2);
            validate_failure(value)
        },
        "2";
    an_invalid_node_id_is_reported => ConfigNodeIdInvalid,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["node_id"] = serde_json::json!("not a node id");
            validate_failure(value)
        },
        "not a node id";
    an_unparsable_bind_address_is_reported => ConfigBindAddressInvalid,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["bind"] = serde_json::json!("not-an-address");
            validate_failure(value)
        },
        "not-an-address";
    an_out_of_range_execution_limit_is_reported => ConfigMaxActiveExecutionsOutOfRange,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["max_active_executions"] = serde_json::json!(0);
            validate_failure(value)
        },
        "0";
    an_out_of_range_lease_ttl_is_reported => ConfigLeaseTtlOutOfRange,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["lease_ttl_seconds"] = serde_json::json!(0);
            validate_failure(value)
        },
        "0";
    a_zero_blob_quota_is_reported => ConfigBlobQuotaInvalid,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["blob_quota_bytes"] = serde_json::json!(0);
            validate_failure(value)
        },
        "0";
    a_zero_workspace_quota_is_reported => ConfigWorkspaceQuotaInvalid,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["workspace_quota_bytes"] = serde_json::json!(0);
            validate_failure(value)
        },
        "0";
    an_empty_client_grant_set_is_reported => ConfigClientGrantCountOutOfRange,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"] = serde_json::json!([]);
            validate_failure(value)
        },
        "0";
    an_invalid_principal_id_is_reported => ConfigPrincipalIdInvalid,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["principal_id"] = serde_json::json!("not a principal");
            validate_failure(value)
        },
        "not a principal";
    a_malformed_client_fingerprint_is_reported => ConfigClientFingerprintInvalid,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["certificate_sha256"] = serde_json::json!("not-a-fingerprint");
            validate_failure(value)
        },
        "the client certificate fingerprint";
    a_duplicated_client_fingerprint_is_reported => ConfigClientFingerprintDuplicate,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            let grant = value["clients"][0].clone();
            value["clients"] = serde_json::json!([grant.clone(), grant]);
            validate_failure(value)
        },
        "the client certificate fingerprint";
    an_empty_client_operation_list_is_reported => ConfigClientOperationsEmpty,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["operations"] = serde_json::json!([]);
            validate_failure(value)
        },
        "the client grant's operation list";
    an_unknown_client_operation_is_reported => ConfigClientOperationUnknown,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["operations"] = serde_json::json!(["fly"]);
            validate_failure(value)
        },
        "fly";

    // Resource scopes.
    too_many_resource_scopes_are_reported => ConfigResourceScopeCountOutOfRange,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["operations"] = serde_json::json!(["blob-read"]);
            let scopes: Vec<serde_json::Value> = (0..65)
                .map(|index| serde_json::json!({
                    "operation": "blob-read",
                    "resource_kind": "blob",
                    "resource_id": format!("{:064x}", index)
                }))
                .collect();
            value["clients"][0]["resource_scopes"] = serde_json::json!(scopes);
            validate_failure(value)
        },
        "the client grant's resource scopes";
    an_unknown_scope_operation_is_reported => ConfigResourceScopeOperationUnknown,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["operations"] = serde_json::json!(["blob-read"]);
            value["clients"][0]["resource_scopes"] = serde_json::json!([{
                "operation": "blob-readd",
                "resource_kind": "blob",
                "resource_id": "cd".repeat(32)
            }]);
            validate_failure(value)
        },
        "blob-readd";
    an_unknown_scope_resource_kind_is_reported => ConfigResourceScopeKindUnknown,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["operations"] = serde_json::json!(["blob-read"]);
            value["clients"][0]["resource_scopes"] = serde_json::json!([{
                "operation": "blob-read",
                "resource_kind": "blobs",
                "resource_id": "cd".repeat(32)
            }]);
            validate_failure(value)
        },
        "blobs";
    a_scope_for_an_operation_the_grant_does_not_hold_is_reported => ConfigResourceScopeNotHeld,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["resource_scopes"] = serde_json::json!([{
                "operation": "blob-read",
                "resource_kind": "blob",
                "resource_id": "cd".repeat(32)
            }]);
            validate_failure(value)
        },
        "blob-read";
    a_scope_for_a_kind_the_operation_never_names_is_reported => ConfigResourceScopeKindNotApplicable,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["resource_scopes"] = serde_json::json!([{
                "operation": "status",
                "resource_kind": "blob",
                "resource_id": "cd".repeat(32)
            }]);
            validate_failure(value)
        },
        "blob";
    a_malformed_scope_identifier_is_reported => ConfigResourceScopeIdentifierInvalid,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["operations"] = serde_json::json!(["blob-read"]);
            value["clients"][0]["resource_scopes"] = serde_json::json!([{
                "operation": "blob-read",
                "resource_kind": "blob",
                "resource_id": "not-a-digest"
            }]);
            validate_failure(value)
        },
        "the resource scope's identifier";
    a_duplicated_resource_scope_is_reported => ConfigResourceScopeDuplicate,
        |root: &Path| {
            let mut value = config_json(root, "127.0.0.1:0");
            value["clients"][0]["operations"] = serde_json::json!(["blob-read"]);
            let scope = serde_json::json!({
                "operation": "blob-read",
                "resource_kind": "blob",
                "resource_id": "cd".repeat(32)
            });
            value["clients"][0]["resource_scopes"] = serde_json::json!([scope.clone(), scope]);
            validate_failure(value)
        },
        "the resource scope";

    // The TLS material.
    an_unreadable_certificate_file_is_reported => TlsCertificateUnreadable,
        |root: &Path| {
            let mut config = installed(root);
            config.tls.certificate_chain = root.join("absent-chain.pem");
            config.validate().unwrap_err()
        },
        "absent-chain.pem";
    a_certificate_that_is_not_a_regular_file_is_reported => TlsCertificateNotARegularFile,
        |root: &Path| {
            let mut config = installed(root);
            config.tls.certificate_chain = root.join("a-directory");
            std::fs::create_dir_all(root.join("a-directory")).unwrap();
            config.validate().unwrap_err()
        },
        "a-directory";
    a_world_writable_certificate_file_is_reported => TlsCertificateWorldWritable,
        |root: &Path| {
            let config = installed(root);
            set_mode(&root.join("server.pem"), 0o666);
            config.validate().unwrap_err()
        },
        "server.pem";
    a_malformed_certificate_file_is_reported => TlsCertificateMalformed,
        |root: &Path| {
            let config = installed(root);
            std::fs::write(root.join("server.pem"), b"not a certificate").unwrap();
            set_mode(&root.join("server.pem"), 0o644);
            config.validate().unwrap_err()
        },
        "server.pem";
    an_unreadable_private_key_is_reported => TlsPrivateKeyUnreadable,
        |root: &Path| {
            let mut config = installed(root);
            config.tls.private_key = root.join("absent-key.pem");
            config.validate().unwrap_err()
        },
        "the private key";
    a_private_key_that_is_not_a_regular_file_is_reported => TlsPrivateKeyNotARegularFile,
        |root: &Path| {
            let mut config = installed(root);
            config.tls.private_key = root.join("a-directory");
            std::fs::create_dir_all(root.join("a-directory")).unwrap();
            config.validate().unwrap_err()
        },
        "the private key";
    a_world_writable_private_key_is_reported => TlsPrivateKeyWorldWritable,
        |root: &Path| {
            let config = installed(root);
            set_mode(&root.join("server-key.pem"), 0o666);
            config.validate().unwrap_err()
        },
        "the private key";
    a_private_key_others_may_read_is_reported => TlsPrivateKeyReadableByOthers,
        |root: &Path| {
            let config = installed(root);
            set_mode(&root.join("server-key.pem"), 0o644);
            config.validate().unwrap_err()
        },
        "the private key";
    a_private_key_the_node_cannot_write_is_reported => TlsPrivateKeyNotWritable,
        |root: &Path| {
            let config = installed(root);
            set_mode(&root.join("server-key.pem"), 0o400);
            config.validate().unwrap_err()
        },
        "the private key";
    a_malformed_private_key_is_reported => TlsPrivateKeyMalformed,
        |root: &Path| {
            let config = installed(root);
            std::fs::write(root.join("server-key.pem"), b"not a private key").unwrap();
            set_mode(&root.join("server-key.pem"), 0o600);
            config.validate().unwrap_err()
        },
        "the private key";
    a_certificate_and_key_that_do_not_match_are_reported => TlsIdentityInvalid,
        |root: &Path| {
            let config = installed(root);
            let other = rcgen::KeyPair::generate().unwrap();
            std::fs::write(root.join("server-key.pem"), other.serialize_pem()).unwrap();
            set_mode(&root.join("server-key.pem"), 0o600);
            config.validate().unwrap_err()
        },
        "the TLS identity";
    a_client_ca_bundle_the_node_cannot_use_is_reported => TlsClientCaInvalid,
        |root: &Path| {
            let config = installed(root);
            let ca = std::fs::read_to_string(root.join("client-ca.pem")).unwrap();
            std::fs::write(root.join("client-ca.pem"), ca.repeat(257)).unwrap();
            set_mode(&root.join("client-ca.pem"), 0o644);
            config.validate().unwrap_err()
        },
        "client-ca.pem";

    // The node's state directories.
    a_state_root_that_is_not_a_directory_is_reported => StateRootNotADirectory,
        |root: &Path| {
            let config = installed(root);
            std::fs::remove_dir_all(&config.execution_root).unwrap();
            config.validate().unwrap_err()
        },
        "execution";
    #[cfg(unix)]
    a_symlinked_state_root_is_reported => StateRootSymlinked,
        |root: &Path| {
            let config = installed(root);
            let real = root.join("real-execution");
            std::fs::create_dir_all(&real).unwrap();
            set_mode(&real, 0o700);
            std::fs::remove_dir_all(&config.execution_root).unwrap();
            std::os::unix::fs::symlink(&real, &config.execution_root).unwrap();
            config.validate().unwrap_err()
        },
        "execution";
    a_state_root_that_is_not_owner_only_is_reported => StateRootNotOwnerOnly,
        |root: &Path| {
            let config = installed(root);
            set_mode(&config.execution_root, 0o755);
            config.validate().unwrap_err()
        },
        "execution";
    overlapping_state_roots_are_reported => StateRootsOverlap,
        |root: &Path| {
            let mut config = installed(root);
            config.blob_root = config.workspace_root.clone();
            config.validate().unwrap_err()
        },
        "workspaces";
    a_database_directory_that_is_not_owner_only_is_reported => DatabaseDirectoryNotOwnerOnly,
        |root: &Path| {
            let config = installed(root);
            set_mode(config.database_path.parent().unwrap(), 0o755);
            config.validate().unwrap_err()
        },
        "state";
    a_database_that_is_not_a_regular_file_is_reported => DatabaseFileNotARegularFile,
        |root: &Path| {
            let config = installed(root);
            std::fs::create_dir_all(&config.database_path).unwrap();
            config.validate().unwrap_err()
        },
        "executions.sqlite";
    a_database_the_node_cannot_write_is_reported => DatabaseFileNotWritable,
        |root: &Path| {
            let config = installed(root);
            std::fs::write(&config.database_path, b"").unwrap();
            set_mode(&config.database_path, 0o400);
            config.validate().unwrap_err()
        },
        "executions.sqlite";

    // Everything else.
    a_misconfigured_host_telemetry_endpoint_is_reported => HostTelemetryEndpointInvalid,
        |root: &Path| {
            let mut config = installed(root);
            config.host_telemetry = Some(eggwork_server::operations::HostTelemetrySettings {
                endpoint: "http://example.com:18080/v2/status".into(),
                refresh_interval_seconds: 15,
                freshness_seconds: 60,
            });
            config.validate().unwrap_err()
        },
        "the host-telemetry endpoint";
}

/// The sandbox helper's trust failures name their own rule.
///
/// The helper is not a configuration *field*: it is a file the configuration
/// points at, and the runner owns the trust decision. `doctor` reports the
/// rule the trust check failed, so an operator is told whether the helper is
/// missing, in a directory that cannot be trusted, or under an ancestor that
/// is world-writable.
#[tokio::test]
async fn the_sandbox_helper_trust_failures_name_their_own_rule() {
    let temp = TempDir::new().unwrap();
    let mut config = installed(temp.path());

    // A helper that is not there.
    config.sandbox_helper = Some(temp.path().join("absent-helper"));
    let detail = sandbox_detail(&config).await;
    assert!(detail.contains("sandbox_helper_unavailable"), "{detail}");

    // A helper the node cannot execute.
    let helper = temp.path().join("helper");
    std::fs::write(&helper, b"#!/bin/sh\n").unwrap();
    set_mode(&helper, 0o600);
    config.sandbox_helper = Some(helper.clone());
    let detail = sandbox_detail(&config).await;
    assert!(detail.contains("sandbox_helper_not_trusted"), "{detail}");

    // A helper in a directory other users may write to.
    let directory = temp.path().join("loose-directory");
    std::fs::create_dir_all(&directory).unwrap();
    set_mode(&directory, 0o777);
    let helper = directory.join("helper");
    std::fs::write(&helper, b"#!/bin/sh\n").unwrap();
    set_mode(&helper, 0o755);
    config.sandbox_helper = Some(helper);
    let detail = sandbox_detail(&config).await;
    assert!(
        detail.contains("sandbox_helper_directory_not_trusted"),
        "{detail}"
    );

    // A helper under an ancestor other users may write to.
    let ancestor = temp.path().join("loose-ancestor");
    std::fs::create_dir_all(&ancestor).unwrap();
    set_mode(&ancestor, 0o777);
    let directory = ancestor.join("bin");
    std::fs::create_dir_all(&directory).unwrap();
    set_mode(&directory, 0o700);
    let helper = directory.join("helper");
    std::fs::write(&helper, b"#!/bin/sh\n").unwrap();
    set_mode(&helper, 0o755);
    config.sandbox_helper = Some(helper);
    let detail = sandbox_detail(&config).await;
    assert!(
        detail.contains("sandbox_helper_ancestor_not_trusted"),
        "{detail}"
    );
}

/// The `doctor` report's sandbox-helper check detail.
async fn sandbox_detail(config: &OperatorConfig) -> String {
    let report = eggwork_server::operations::doctor(config).await;
    let report = serde_json::to_value(report).expect("the doctor report serialises");
    report["checks"]
        .as_array()
        .expect("the doctor report carries checks")
        .iter()
        .find(|check| check["name"] == "sandbox-resource-backend")
        .map(|check| check["detail"].as_str().unwrap_or_default().to_owned())
        .expect("the doctor report carries a sandbox check")
}

/// A configuration fault never names a secret.
///
/// The private key is named by its role and never by its path; the client's
/// certificate fingerprint is named by its role and never by its value; and
/// no certificate subject is ever echoed. The rule and the role are enough to
/// act on.
#[test]
fn a_configuration_fault_never_names_the_private_key_the_fingerprint_or_a_subject() {
    let temp = TempDir::new().unwrap();
    let config_path = install(temp.path(), "127.0.0.1:0");
    let key_path = temp.path().join("server-key.pem");
    let fingerprint = "ab".repeat(32);
    let secrets = [
        key_path.to_str().unwrap().to_owned(),
        fingerprint.clone(),
        "localhost".to_owned(),
        "eggwork operator test CA".to_owned(),
    ];

    // A private key that others may read.
    set_mode(&key_path, 0o644);
    let fault = fault_of(
        OperatorConfig::load(&config_path)
            .unwrap()
            .validate()
            .unwrap_err(),
    );
    assert_eq!(fault.rule, ConfigRule::TlsPrivateKeyReadableByOthers);
    let message = fault.to_string();
    assert!(message.contains("the private key"), "{message}");
    for secret in &secrets {
        assert!(
            !message.contains(secret),
            "the fault leaked {secret}: {message}"
        );
    }

    // A client fingerprint that is not a fingerprint.
    let mut value = config_json(temp.path(), "127.0.0.1:0");
    value["clients"][0]["certificate_sha256"] = serde_json::json!("not-a-fingerprint");
    let config = serde_json::from_value::<OperatorConfig>(value).unwrap();
    let fault = fault_of(config.validate().unwrap_err());
    assert_eq!(fault.rule, ConfigRule::ConfigClientFingerprintInvalid);
    let message = fault.to_string();
    for secret in &secrets {
        assert!(
            !message.contains(secret),
            "the fault leaked {secret}: {message}"
        );
    }
}

/// Every operator surface reports the same rule for the same fault.
///
/// `config print`, `config validate`, `doctor`, and the daemon's startup
/// failure are four ways an operator meets the same configuration, and they
/// must not describe the same fault four different ways.
#[test]
fn every_operator_surface_reports_the_same_rule_for_the_same_fault() {
    let temp = TempDir::new().unwrap();
    let config_path = install(temp.path(), "127.0.0.1:0");
    let path = config_path.to_str().unwrap();

    // A fault every surface meets: the configuration file itself.
    set_mode(&config_path, 0o666);
    for verb in [
        vec!["config", "print"],
        vec!["config", "validate"],
        vec!["doctor"],
        vec!["run"],
    ] {
        let mut args = verb.clone();
        args.push("--config");
        args.push(path);
        let (stdout, stderr, ok) = eggworkd(&args);
        assert!(!ok, "{} accepted a world-writable configuration", verb[0]);
        let reported = format!("{stdout}{stderr}");
        assert!(
            reported.contains("config_file_world_writable"),
            "{} did not name the rule: {reported}",
            verb.join(" ")
        );
    }
    set_mode(&config_path, 0o644);

    // A fault the surfaces that validate meet: the bind address.
    let mut value = config_json(temp.path(), "127.0.0.1:0");
    value["bind"] = serde_json::json!("not-an-address");
    std::fs::write(&config_path, serde_json::to_vec(&value).unwrap()).unwrap();
    set_mode(&config_path, 0o644);

    let (stdout, stderr, ok) = eggworkd(&["config", "validate", "--config", path]);
    assert!(
        !ok,
        "config validate accepted an unparsable bind address: {stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        configuration_detail(&report).contains("config_bind_address_invalid"),
        "config validate did not name the rule: {stdout}"
    );

    let (stdout, stderr, ok) = eggworkd(&["doctor", "--config", path]);
    assert!(!ok, "doctor accepted an unparsable bind address: {stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        configuration_detail(&report).contains("config_bind_address_invalid"),
        "doctor did not name the rule: {stdout}"
    );

    let (_, stderr, ok) = eggworkd(&["run", "--config", path]);
    assert!(!ok, "the daemon accepted an unparsable bind address");
    assert!(
        stderr.contains("config_bind_address_invalid"),
        "the daemon's startup failure did not name the rule: {stderr}"
    );
}

/// A valid configuration still loads, and still prints redacted.
///
/// The added detail applies to faults. The values `config print` prints are
/// unchanged: the private key path and the client fingerprints are still
/// redacted.
#[test]
fn a_valid_configuration_still_loads_and_still_prints_redacted() {
    let temp = TempDir::new().unwrap();
    let config_path = install(temp.path(), "127.0.0.1:0");
    let path = config_path.to_str().unwrap();

    let config = OperatorConfig::load(&config_path).expect("the configuration loads");
    config.validate().expect("the configuration validates");

    let (stdout, stderr, ok) = eggworkd(&["config", "print", "--config", path]);
    assert!(ok, "config print failed: {stderr}");
    assert!(stdout.contains("[REDACTED]"), "{stdout}");
    assert!(!stdout.contains("server-key.pem"), "{stdout}");
    assert!(!stdout.contains(&"ab".repeat(32)), "{stdout}");

    let (stdout, stderr, ok) = eggworkd(&["config", "validate", "--config", path]);
    assert!(ok, "config validate failed: {stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["valid"], serde_json::json!(true), "{stdout}");
}
