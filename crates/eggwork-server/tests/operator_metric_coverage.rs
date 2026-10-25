//! The operator surface reports every metric the node can increment.
//!
//! These tests read the *report path* only: they write rows into the node's
//! metrics table and read the counters back. They are written that way on
//! purpose, because it is what lets them run against the code as it was
//! before the metric vocabulary moved to the producers — they are evidence
//! about the report, not a restatement of the fix.
//!
//! The report used to seed its counter map from a hand-maintained list of
//! names and copy a stored value *only* when the name was already in that
//! map. A counter the server incremented but the list did not name was
//! therefore incremented on real traffic and dropped by the report: five of
//! them, all of them workspace counters. A test that asserted only that the
//! current five are present would not have caught that, and — more to the
//! point — would not have failed against the pre-change code either.

use eggwork_server::operations::{OperatorConfig, metrics_snapshot};
use std::path::Path;
use tempfile::TempDir;

/// The counters the workspace derivation path increments.
///
/// These are the five names the server incremented and the operator surface
/// dropped.
const WORKSPACE_COUNTERS: [&str; 5] = [
    "workspace_manifest_registrations",
    "workspace_derived_requests",
    "workspace_derived_hits",
    "workspace_patch_entries",
    "workspace_base_missing",
];

/// A configuration document in the shape the loader accepts.
fn config_json(root: &Path) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "node_id": "node-test",
        "bind": "127.0.0.1:0",
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

fn config(root: &Path) -> OperatorConfig {
    serde_json::from_value(config_json(root)).expect("config")
}

/// A node whose metrics table holds the given counters.
fn node_with_metrics(root: &Path, rows: &[(&str, i64)]) -> OperatorConfig {
    let config = config(root);
    std::fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
    let db = rusqlite::Connection::open(&config.database_path).unwrap();
    db.execute_batch("CREATE TABLE node_metrics(name TEXT PRIMARY KEY,value INTEGER);")
        .unwrap();
    for (name, value) in rows {
        db.execute(
            "INSERT INTO node_metrics VALUES (?1,?2)",
            rusqlite::params![name, value],
        )
        .unwrap();
    }
    drop(db);
    config
}

/// Every counter the server increments reaches the operator report.
///
/// The counters are written into the node's metrics table exactly as the
/// server writes them, and the report is read back through the operator's own
/// path. A report that drops a counter the server increments fails here.
#[test]
fn the_operator_report_carries_every_metric_the_server_can_increment() {
    let temp = TempDir::new().unwrap();
    let rows: Vec<(&str, i64)> = WORKSPACE_COUNTERS
        .iter()
        .enumerate()
        .map(|(index, name)| (*name, index as i64 + 1))
        .collect();
    let config = node_with_metrics(temp.path(), &rows);
    let metrics = metrics_snapshot(&config).unwrap();
    for (name, value) in &rows {
        assert_eq!(
            metrics.counters.get(*name),
            Some(&(*value as u64)),
            "{name} is incremented by the server but missing from the operator report"
        );
    }
}

/// A row the report does not recognise is reported, not dropped.
///
/// A node whose metrics table was written by another version of the node must
/// still serve `eggworkd metrics`: the report may not know every name, but it
/// may not lose one.
#[test]
fn a_metric_name_the_report_does_not_know_is_reported_rather_than_dropped() {
    let temp = TempDir::new().unwrap();
    let config = node_with_metrics(temp.path(), &[("a_counter_from_another_version", 11)]);
    let metrics = metrics_snapshot(&config).unwrap();
    assert_eq!(
        metrics.counters.get("a_counter_from_another_version"),
        Some(&11),
        "a counter the report does not recognise must still be reported"
    );
}
