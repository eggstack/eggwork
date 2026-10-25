//! Fixtures shared by the operator-surface tests.
//!
//! The helpers live in one place because the tests that use them are
//! deliberately version-independent: they drive the operator binary and read
//! its JSON, so they can be run against the code as it was before a change as
//! well as against the code as it is now. That is what makes them evidence
//! rather than a restatement of the fix.

#![allow(dead_code)]

use eggwork_server::operations::OperatorConfig;
use std::path::{Path, PathBuf};

/// The counters the workspace derivation path increments.
pub const WORKSPACE_COUNTERS: [&str; 5] = [
    "workspace_manifest_registrations",
    "workspace_derived_requests",
    "workspace_derived_hits",
    "workspace_patch_entries",
    "workspace_base_missing",
];

/// The facts the wire status response and the operator's status payload share.
///
/// A key that appears on both sides under the same name must carry the same
/// fact on both sides. `active_executions` meant permits held on the wire and
/// non-terminal rows in the operator's listing, and that collision is what
/// this table exists to make impossible.
pub const SHARED_FACTS: [(&str, &str); 2] = [("node_id", "node_id"), ("draining", "draining")];

/// The key names the wire status response publishes.
///
/// `active_executions` is the wire's name for the *permits held* count. It is
/// a published name: a deployed `eggwork-client` deserializes it. The operator
/// surface may not reuse it for a different fact.
pub const WIRE_STATUS_KEYS: [&str; 4] =
    ["node_id", "draining", "active_executions", "capabilities"];

/// A configuration document in the shape the loader accepts.
pub fn config_json(root: &Path, bind: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "node_id": "node-test",
        "bind": bind,
        "execution_root": root.join("execution"),
        "database_path": root.join("state/executions.sqlite"),
        "blob_root": root.join("blobs"),
        "blob_quota_bytes": 4096,
        "workspace_root": root.join("workspaces"),
        "workspace_quota_bytes": 4096,
        "max_active_executions": 2,
        "lease_ttl_seconds": 60,
        "tls": {
            "certificate_chain": root.join("server.pem"),
            "private_key": root.join("server-key.pem"),
            "client_ca": root.join("client-ca.pem")
        },
        "clients": [{
            "principal_id": "controller",
            "certificate_sha256": "ab".repeat(32),
            "operations": ["status"]
        }]
    })
}

/// Install a configuration with real TLS material and owner-only directories.
///
/// The certificates carry the subjects `localhost` and `eggwork operator test
/// CA`: the redaction test asserts that neither ever reaches a configuration
/// fault message.
pub fn install(root: &Path, bind: &str) -> PathBuf {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params =
        rcgen::CertificateParams::new(vec!["eggwork operator test CA".into()]).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let server_key = rcgen::KeyPair::generate().unwrap();
    let server_params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
    let server = server_params.signed_by(&server_key, &ca, &ca_key).unwrap();
    std::fs::write(root.join("server.pem"), server.pem()).unwrap();
    std::fs::write(root.join("client-ca.pem"), ca.pem()).unwrap();
    let key_path = root.join("server-key.pem");
    std::fs::write(&key_path, server_key.serialize_pem()).unwrap();
    for name in ["execution", "blobs", "workspaces", "state"] {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        set_mode(&path, 0o700);
    }
    set_mode(&key_path, 0o600);
    set_mode(&root.join("server.pem"), 0o644);
    set_mode(&root.join("client-ca.pem"), 0o644);
    let config_path = root.join("node.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(&config_json(root, bind)).unwrap(),
    )
    .unwrap();
    set_mode(&config_path, 0o644);
    config_path
}

/// The installed fixture's configuration, loaded into memory.
pub fn installed(root: &Path) -> OperatorConfig {
    install(root, "127.0.0.1:0");
    serde_json::from_value(config_json(root, "127.0.0.1:0")).expect("config")
}

#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
pub fn set_mode(_: &Path, _: u32) {}

/// Run the operator binary and return its stdout, stderr, and exit status.
pub fn eggworkd(args: &[&str]) -> (String, String, bool) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_eggworkd"))
        .args(args)
        .output()
        .expect("the eggworkd binary");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

/// A port that was free a moment ago.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The `doctor` report's configuration check detail.
///
/// `config validate` wraps the report in a `doctor` envelope and `doctor`
/// prints the report itself; both carry the same checks.
pub fn configuration_detail(report: &serde_json::Value) -> String {
    let checks = report["doctor"]["checks"]
        .as_array()
        .or_else(|| report["checks"].as_array())
        .expect("the doctor report carries checks");
    checks
        .iter()
        .find(|check| check["name"] == "configuration")
        .map(|check| check["detail"].as_str().unwrap_or_default().to_owned())
        .expect("the doctor report carries a configuration check")
}
