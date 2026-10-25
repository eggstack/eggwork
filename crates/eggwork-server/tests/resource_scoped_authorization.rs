//! Resource-scoped authorization: the configuration shape, the decision rule,
//! and the boundaries the decision may not cross.
//!
//! These tests drive the authorizer through the same public surface the
//! daemon uses — the operator configuration's grant table — so they need no
//! filesystem, no process, no TLS handshake, and no host platform. The
//! end-to-end proof that a scope cannot reach another principal's resource
//! lives in the server's own test module, which starts a real node; this
//! file pins the decision itself.
//!
//! Nothing in this file may use `/bin/sh`, `std::os::unix`, or a Unix-only
//! mode: the target is compiled and run by the Windows job in CI.

use eggwork_core::{BlobDigest, ExecutionId, PrincipalId, WorkspaceId};
use eggwork_server::{
    Authorizer, NodePrincipal, Operation, OperationRequest, ResourceId,
    operations::{ClientGrant, GrantAuthorizer, OperatorConfig, ResourceScope, TlsFiles},
};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

/// Build the node's grant table from the client grants.
///
/// `OperatorConfig::principals` reads only the client grants: the paths and
/// the TLS material are never touched, so a grant table can be built here
/// without a node, a certificate, or a directory.
fn grant_table(clients: Vec<ClientGrant>) -> GrantAuthorizer {
    OperatorConfig {
        schema_version: 1,
        node_id: "node-test".into(),
        bind: "127.0.0.1:0".into(),
        execution_root: PathBuf::from("unused"),
        database_path: PathBuf::from("unused"),
        blob_root: PathBuf::from("unused"),
        blob_quota_bytes: 1,
        workspace_root: PathBuf::from("unused"),
        workspace_quota_bytes: 1,
        max_active_executions: 1,
        lease_ttl_seconds: 1,
        sandbox_helper: None,
        tls: TlsFiles {
            certificate_chain: PathBuf::from("unused"),
            private_key: PathBuf::from("unused"),
            client_ca: PathBuf::from("unused"),
        },
        clients,
        host_telemetry: None,
    }
    .principals()
    .expect("the grant table builds")
    .1
}

fn client(
    principal: &str,
    fingerprint: u64,
    operations: &[&str],
    resource_scopes: Vec<ResourceScope>,
) -> ClientGrant {
    ClientGrant {
        principal_id: principal.into(),
        certificate_sha256: format!("{fingerprint:064x}"),
        operations: operations.iter().map(|name| (*name).to_string()).collect(),
        resource_scopes,
    }
}

fn scope(operation: &str, resource_kind: &str, resource_id: &str) -> ResourceScope {
    ResourceScope {
        operation: operation.into(),
        resource_kind: resource_kind.into(),
        resource_id: resource_id.into(),
    }
}

fn principal(name: &str) -> NodePrincipal {
    NodePrincipal {
        id: PrincipalId::new(name.to_owned()).expect("principal id"),
    }
}

fn request(operation: Operation, resource: Option<ResourceId>) -> OperationRequest {
    OperationRequest {
        operation,
        resource,
    }
}

fn digest_a() -> String {
    "a".repeat(64)
}

fn digest_b() -> String {
    "b".repeat(64)
}

fn blob(value: &str) -> ResourceId {
    ResourceId::Blob(BlobDigest::parse(value.to_owned()).expect("blob digest"))
}

fn execution(value: &str) -> ResourceId {
    ResourceId::Execution(ExecutionId::new(value.to_owned()).expect("execution id"))
}

fn workspace(value: &str) -> ResourceId {
    ResourceId::Workspace(WorkspaceId::new(value.to_owned()).expect("workspace id"))
}

/// A grant that carries no scope keeps exactly its operation-level meaning.
#[test]
fn a_grant_without_resource_scopes_keeps_its_operation_level_meaning() {
    let authorizer = grant_table(vec![client(
        "controller",
        1,
        &["status", "blob-read"],
        vec![],
    )]);
    let controller = principal("controller");
    assert!(authorizer.authorize_request(&controller, &request(Operation::Status, None)));
    assert!(authorizer.authorize_request(
        &controller,
        &request(Operation::BlobRead, Some(blob(&digest_a())))
    ));
    assert!(!authorizer.authorize_request(&controller, &request(Operation::Execute, None)));
}

/// Two principals, the same operations, different scopes: the scope is what
/// separates them, and the operation grant alone does not.
#[test]
fn two_principals_with_the_same_operations_and_different_scopes_get_different_answers() {
    let authorizer = grant_table(vec![
        client(
            "controller-a",
            1,
            &["blob-read"],
            vec![scope("blob-read", "blob", &digest_a())],
        ),
        client(
            "controller-b",
            2,
            &["blob-read"],
            vec![scope("blob-read", "blob", &digest_b())],
        ),
    ]);
    let a = principal("controller-a");
    let b = principal("controller-b");
    assert!(
        authorizer.authorize_request(&a, &request(Operation::BlobRead, Some(blob(&digest_a()))))
    );
    assert!(
        !authorizer.authorize_request(&a, &request(Operation::BlobRead, Some(blob(&digest_b()))))
    );
    assert!(
        authorizer.authorize_request(&b, &request(Operation::BlobRead, Some(blob(&digest_b()))))
    );
    assert!(
        !authorizer.authorize_request(&b, &request(Operation::BlobRead, Some(blob(&digest_a()))))
    );
}

/// The scope is consulted. Without this the decision would be the
/// operation-level grant alone, and the digest the scope does not name would
/// be allowed.
#[test]
fn a_resource_scope_narrows_the_operation_grant_it_names() {
    let authorizer = grant_table(vec![client(
        "controller",
        1,
        &["blob-read"],
        vec![scope("blob-read", "blob", &digest_a())],
    )]);
    let controller = principal("controller");
    // The operation grant alone would allow both digests.
    assert!(authorizer.authorize(&controller, Operation::BlobRead));
    // The scope refuses the digest it does not name, and allows the one it
    // does.
    assert!(!authorizer.authorize_request(
        &controller,
        &request(Operation::BlobRead, Some(blob(&digest_b())))
    ));
    assert!(authorizer.authorize_request(
        &controller,
        &request(Operation::BlobRead, Some(blob(&digest_a())))
    ));
}

/// A scope cannot grant an operation the principal does not hold — not even
/// when the grant list is misconfigured to name an operation it does not
/// hold.
#[test]
fn a_scope_never_grants_an_operation_the_principal_does_not_hold() {
    let authorizer = grant_table(vec![client(
        "controller",
        1,
        &["status"],
        vec![scope("blob-read", "blob", &digest_a())],
    )]);
    let controller = principal("controller");
    assert!(!authorizer.authorize_request(
        &controller,
        &request(Operation::BlobRead, Some(blob(&digest_a())))
    ));
    assert!(!authorizer.authorize_request(&controller, &request(Operation::BlobRead, None)));
}

/// A scope narrows the operation *and* the resource kind it names, and only
/// those. `blob-write` has no scope of its own, so it falls back to the
/// operation grant.
#[test]
fn a_scope_narrows_only_the_operation_and_kind_it_names() {
    let authorizer = grant_table(vec![client(
        "controller",
        1,
        &["blob-read", "blob-write"],
        vec![scope("blob-read", "blob", &digest_a())],
    )]);
    let controller = principal("controller");
    assert!(authorizer.authorize_request(
        &controller,
        &request(Operation::BlobRead, Some(blob(&digest_a())))
    ));
    assert!(!authorizer.authorize_request(
        &controller,
        &request(Operation::BlobRead, Some(blob(&digest_b())))
    ));
    assert!(authorizer.authorize_request(
        &controller,
        &request(Operation::BlobWrite, Some(blob(&digest_a())))
    ));
    assert!(authorizer.authorize_request(
        &controller,
        &request(Operation::BlobWrite, Some(blob(&digest_b())))
    ));
}

/// A request that carries no resource falls back to the operation-level
/// grant, exactly as it did before resource scopes existed.
#[test]
fn a_request_without_a_resource_falls_back_to_the_operation_grant() {
    let authorizer = grant_table(vec![client(
        "controller",
        1,
        &["blob-read"],
        vec![scope("blob-read", "blob", &digest_a())],
    )]);
    let controller = principal("controller");
    assert!(authorizer.authorize_request(&controller, &request(Operation::BlobRead, None)));
}

/// `execute` names two kinds — the execution and the workspace — and a scope
/// for one does not narrow the other.
#[test]
fn an_execute_scope_names_the_execution_and_the_workspace_separately() {
    let authorizer = grant_table(vec![client(
        "controller",
        1,
        &["execute"],
        vec![
            scope("execute", "execution", "exec-1"),
            scope("execute", "workspace", "ws-1"),
        ],
    )]);
    let controller = principal("controller");
    assert!(authorizer.authorize_request(
        &controller,
        &request(Operation::Execute, Some(execution("exec-1")))
    ));
    assert!(!authorizer.authorize_request(
        &controller,
        &request(Operation::Execute, Some(execution("exec-2")))
    ));
    assert!(authorizer.authorize_request(
        &controller,
        &request(Operation::Execute, Some(workspace("ws-1")))
    ));
    assert!(!authorizer.authorize_request(
        &controller,
        &request(Operation::Execute, Some(workspace("ws-2")))
    ));
}

/// The scope's identifier is the string form of the typed resource, so the
/// configuration and the request path agree on what an identifier is.
#[test]
fn a_scope_identifier_is_the_string_form_of_the_typed_resource() {
    let authorizer = grant_table(vec![client(
        "controller",
        1,
        &["observe"],
        vec![scope("observe", "execution", "exec-1")],
    )]);
    let controller = principal("controller");
    let resolved = ResourceId::Execution(ExecutionId::new("exec-1".to_owned()).unwrap());
    assert!(
        authorizer.authorize_request(&controller, &request(Operation::Observe, Some(resolved)))
    );
    let other = ResourceId::Execution(ExecutionId::new("exec-2".to_owned()).unwrap());
    assert!(!authorizer.authorize_request(&controller, &request(Operation::Observe, Some(other))));
}

/// The authorizer decides the scope; ownership stays with the store.
///
/// A scope names a resource the principal may not own. The authorizer cannot
/// know who owns it — it consults a configuration-derived set only, and never
/// the store — so it lets the request reach the store, and the store's
/// `principal_id` fence refuses it there. The authorizer must not pretend to
/// be that fence.
#[test]
fn the_authorizer_decides_the_scope_and_leaves_ownership_to_the_store() {
    let authorizer = grant_table(vec![client(
        "controller-b",
        1,
        &["observe"],
        vec![scope("observe", "execution", "exec-owned-by-controller-a")],
    )]);
    let b = principal("controller-b");
    assert!(authorizer.authorize_request(
        &b,
        &request(
            Operation::Observe,
            Some(execution("exec-owned-by-controller-a"))
        )
    ));
}

/// The residual boundary, stated as a test: a broad `blob-read` grant with no
/// scope still reaches every digest. Blob storage is content-addressed and
/// shared, so a blob has no owner to fence it; scoping is the only boundary
/// available for it, and it is opt-in.
#[test]
fn without_a_blob_scope_the_blob_boundary_is_the_operation_grant() {
    let authorizer = grant_table(vec![client("controller", 1, &["blob-read"], vec![])]);
    let controller = principal("controller");
    assert!(authorizer.authorize_request(
        &controller,
        &request(Operation::BlobRead, Some(blob(&digest_b())))
    ));
}

/// A configuration written before resource scopes existed still loads.
#[test]
fn a_configuration_written_before_resource_scopes_still_loads() {
    let grant: ClientGrant = serde_json::from_value(json!({
        "principal_id": "controller",
        "certificate_sha256": format!("{:064x}", 1),
        "operations": ["status"],
    }))
    .expect("the older grant loads");
    assert!(grant.resource_scopes.is_empty());
}

/// An unknown field inside a scope is refused rather than ignored.
#[test]
fn an_unknown_field_in_a_resource_scope_is_refused() {
    let document = json!({
        "principal_id": "controller",
        "certificate_sha256": format!("{:064x}", 1),
        "operations": ["blob-read"],
        "resource_scopes": [{
            "operation": "blob-read",
            "resource_kind": "blob",
            "resource_id": digest_a(),
            "unexpected": true,
        }],
    });
    assert!(serde_json::from_value::<ClientGrant>(document).is_err());
}

/// The shape an older node's `ClientGrant` had: no `resource_scopes` field,
/// and `deny_unknown_fields` on the struct.
///
/// This is a simulation of the older node's deserializer, not an older
/// binary: the refusal it demonstrates is the refusal an older node makes,
/// because the older struct had exactly these fields and exactly this
/// attribute. The fields are declared so the shape matches; this test never
/// reads them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct OlderClientGrant {
    principal_id: String,
    certificate_sha256: String,
    operations: Vec<String>,
}

/// An older node refuses a configuration that carries a resource scope.
///
/// This is correct behaviour, not a defect: a node that silently ignored the
/// narrowing the operator wrote would be a node with a wider boundary than
/// its configuration states.
#[test]
fn an_older_node_refuses_a_configuration_that_carries_a_resource_scope() {
    let document = json!({
        "principal_id": "controller",
        "certificate_sha256": format!("{:064x}", 1),
        "operations": ["blob-read"],
        "resource_scopes": [{
            "operation": "blob-read",
            "resource_kind": "blob",
            "resource_id": digest_a(),
        }],
    });
    // The current node reads it.
    assert!(serde_json::from_value::<ClientGrant>(document.clone()).is_ok());
    // An older node's deserializer refuses it.
    assert!(serde_json::from_value::<OlderClientGrant>(document).is_err());
}
