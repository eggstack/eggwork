//! The operator surface names the two execution facts apart.
//!
//! One name, `active_executions`, carried two facts: the permits the running
//! node holds, and the durable rows that are still in a non-terminal state.
//! The two are different measurements and they legitimately disagree — an
//! execution whose process died keeps its row long after its permit was
//! released, and an idempotent re-request releases its permit while the
//! original execution still holds one.
//!
//! These tests drive the operator binary and read its JSON, and they use no
//! API that the fix introduced, so they can be run against the code as it was
//! before the change as well as against the code as it is now. That is what
//! makes them evidence rather than a restatement of the fix.

mod common;

use common::{SHARED_FACTS, WIRE_STATUS_KEYS, WORKSPACE_COUNTERS, eggworkd, free_port, install};
use eggwork_server::operations::OperatorConfig;
use tempfile::TempDir;

/// The operator status payload carries both execution facts, under two names.
///
/// `execution_permits_held` is how many execution permits the running node
/// holds. `non_terminal_executions` is how many durable rows are still in a
/// non-terminal state. The two are different measurements and an operator
/// investigating a stuck execution needs both.
#[tokio::test]
async fn the_operator_status_payload_carries_both_execution_facts_under_distinct_names() {
    let temp = TempDir::new().unwrap();
    let port = free_port();
    let config_path = install(temp.path(), &format!("127.0.0.1:{port}"));
    let config = OperatorConfig::load(&config_path).unwrap();
    let server = eggwork_server::operations::start_server(&config)
        .await
        .unwrap();
    let (stdout, stderr, ok) = eggworkd(&["status", "--config", config_path.to_str().unwrap()]);
    server.shutdown().await;
    assert!(ok, "status failed: {stderr}");
    let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        payload.get("execution_permits_held").is_some(),
        "the status payload must carry the permits-held count: {payload}"
    );
    assert!(
        payload.get("non_terminal_executions").is_some(),
        "the status payload must carry the non-terminal count: {payload}"
    );
    assert!(
        payload.get("active_executions").is_none(),
        "the status payload must not use the wire's ambiguous name: {payload}"
    );
    // The node is running and holds no permit, so the live count is zero.
    assert_eq!(payload["execution_permits_held"], serde_json::json!(0));
    assert_eq!(payload["non_terminal_executions"], serde_json::json!(0));
}

/// The five workspace counters are visible to the operator.
///
/// These are the counters the server incremented on real traffic and the
/// operator surface dropped.
#[tokio::test]
async fn the_five_workspace_counters_are_visible_in_the_operator_status_payload() {
    let temp = TempDir::new().unwrap();
    let port = free_port();
    let config_path = install(temp.path(), &format!("127.0.0.1:{port}"));
    let config = OperatorConfig::load(&config_path).unwrap();
    let server = eggwork_server::operations::start_server(&config)
        .await
        .unwrap();
    let (stdout, stderr, ok) = eggworkd(&["status", "--config", config_path.to_str().unwrap()]);
    server.shutdown().await;
    assert!(ok, "status failed: {stderr}");
    let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let counters = &payload["metrics"]["counters"];
    for name in WORKSPACE_COUNTERS {
        assert!(
            counters.get(name).is_some(),
            "{name} is incremented by the server but missing from the status payload"
        );
    }
}

/// The execution listing uses the same name for the non-terminal fact.
///
/// The two operator surfaces must agree: an operator who reads the count in
/// the listing and then looks for it in the status payload must find it under
/// the same name.
#[tokio::test]
async fn the_execution_listing_uses_the_same_name_for_the_non_terminal_fact() {
    let temp = TempDir::new().unwrap();
    let port = free_port();
    let config_path = install(temp.path(), &format!("127.0.0.1:{port}"));
    let config = OperatorConfig::load(&config_path).unwrap();
    let server = eggwork_server::operations::start_server(&config)
        .await
        .unwrap();
    let (stdout, stderr, ok) = eggworkd(&[
        "executions",
        "list",
        "--config",
        config_path.to_str().unwrap(),
    ]);
    server.shutdown().await;
    assert!(ok, "executions list failed: {stderr}");
    let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        payload.get("non_terminal_executions").is_some(),
        "the listing must carry the non-terminal count under its own name: {payload}"
    );
    assert!(
        payload.get("active_executions").is_none(),
        "the listing must not use the wire's ambiguous name: {payload}"
    );
}

/// No operator payload reuses a published wire name for a different fact.
///
/// This is the generalisation of the specific defect: the wire status
/// response publishes `active_executions` for the permits-held count, and no
/// operator payload may use that name for anything. The names the two
/// surfaces share must be the names that carry the same fact on both sides.
#[tokio::test]
async fn no_operator_payload_reuses_a_wire_name_for_another_fact() {
    let temp = TempDir::new().unwrap();
    let port = free_port();
    let config_path = install(temp.path(), &format!("127.0.0.1:{port}"));
    let config = OperatorConfig::load(&config_path).unwrap();
    let server = eggwork_server::operations::start_server(&config)
        .await
        .unwrap();
    let (status, _, status_ok) = eggworkd(&["status", "--config", config_path.to_str().unwrap()]);
    let (listing, _, listing_ok) = eggworkd(&[
        "executions",
        "list",
        "--config",
        config_path.to_str().unwrap(),
    ]);
    server.shutdown().await;
    assert!(status_ok && listing_ok);
    for (name, payload) in [
        (
            "status",
            serde_json::from_str::<serde_json::Value>(&status).unwrap(),
        ),
        (
            "executions list",
            serde_json::from_str::<serde_json::Value>(&listing).unwrap(),
        ),
    ] {
        let keys: Vec<&str> = payload
            .as_object()
            .expect("the payload is an object")
            .keys()
            .map(String::as_str)
            .collect();
        for key in &keys {
            assert!(
                !WIRE_STATUS_KEYS.contains(key)
                    || SHARED_FACTS.iter().any(|(shared, _)| shared == key),
                "the {name} payload reuses the wire name {key}, which the wire means by something else"
            );
        }
    }
}

/// The wire status type is unchanged.
///
/// `eggwork_core::NodeStatus` is a published wire type: a deployed client
/// deserializes it. A rename would remove a field those clients require, and
/// a missing field is a deserialization error rather than a defaulted value.
/// This test fails if the wire type changes in any way.
#[test]
fn the_wire_status_type_still_deserializes_the_existing_fields() {
    let payload = serde_json::json!({
        "node_id": "node-test",
        "draining": false,
        "active_executions": 3,
        "capabilities": {
            "protocol": {"min": {"major": 1, "minor": 0}, "max": {"major": 1, "minor": 0}},
            "features": [],
            "max_active_executions": 4
        }
    });
    let status: eggwork_core::NodeStatus =
        serde_json::from_value(payload.clone()).expect("the status response still deserializes");
    assert_eq!(status.active_executions, 3);
    assert_eq!(status.node_id.as_str(), "node-test");
    assert!(!status.draining);
    assert_eq!(status.capabilities.max_active_executions, 4);

    // The wire type carries exactly the four fields it always carried.
    let encoded = serde_json::to_value(&status).unwrap();
    let mut keys: Vec<&str> = encoded
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["active_executions", "capabilities", "draining", "node_id"],
        "the wire status type must not gain or lose a field"
    );

    // A status response without `active_executions` must not deserialize: a
    // rename of the wire field would leave every deployed client that reads
    // node status broken, and this is the check that notices.
    let mut without = payload;
    without.as_object_mut().unwrap().remove("active_executions");
    assert!(
        serde_json::from_value::<eggwork_core::NodeStatus>(without).is_err(),
        "the wire field is required; removing it must break a client"
    );
}
