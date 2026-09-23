# Security M001 Closure — Authorization, Redaction, and Threat Model

Source plan: `plans/implementation/security-isolation-resource/001-authorization-redaction-and-threat-model.md`  
Subsystem roadmap: `plans/subsystems/security-isolation-resource-roadmap.md`  
Threat model: `plans/security/threat-model.md`  
Reviewed implementation commit: `f629f9b`  
Planning/closure commit: pending

## Finding

M001 is closed as the authorization, redaction, and threat-model foundation.
Each implemented HTTP route maps to an operation capability. Authorizers now
receive a typed optional resource ID, while existing operation-wide
authorizers remain compatible. Body-carried execution, workspace, and blob
resource IDs are checked before mutation. Execution observation is owner
fenced and returns the same not-found response for a foreign principal as for
a missing execution. Execution controls/events continue to enforce owner,
generation, and lease requirements; workspace and artifact records remain
principal-fenced.

Wire request DTOs reject unknown fields, so callers cannot inject
`principal_id` or other authority fields. Environment values, stdin, argv,
request metadata, process output, event metadata, client transport details,
client endpoint paths, and startup diagnostics are structurally redacted from
Debug/error formatting. Node identity continues to derive only from the
verified mTLS leaf certificate mapped by the configured resolver.

This closes M001's foundation criteria; it does not qualify arbitrary remote
execution as safe on an unsandboxed host. The node rejects requested isolation
modes it cannot satisfy. Resource requests remain explicitly not enforced in
the current runner backend. Trusted Landlock setup and enforced resource
controls remain Security M002/M003 prerequisites for a broader security
qualification.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Every network operation has an authorization descriptor | `operation_for` maps capabilities, status, execute, observe, cancel, renew, events, blob read/write, workspace creation, and artifact listing/download. `route_contract_is_fixed_target_and_versioned` asserts the complete implemented route matrix; unknown routes are not dispatched. |
| Authorization can use principal and resource IDs | `OperationRequest` carries `Operation` and typed `ResourceId`; path and body resources are passed to `Authorizer::authorize_request`. `resource_aware_authorizer_can_scope_blob_capability` verifies per-digest allow/deny behavior. Existing operation-wide closures retain the default adapter. |
| Denied operations precede side effects | `mtls_authorization_and_fixed_target_execution` denies execution, blob operations, workspace creation, artifacts, observation, cancellation, renewal, and events while allowing status/capabilities. It verifies a denied process does not create its marker. Upload denial occurs at the preflight route before storage. |
| Identity cannot originate from request JSON | `authenticated_principal` requires authenticated TLS context and resolves only verified leaf DER. Execute, control, workspace, and blob JSON DTOs reject unknown fields. `request_payload_cannot_supply_authoritative_principal` checks forged principal fields are rejected. `fingerprint_mapping_uses_leaf_der_only` checks mapping input. |
| Execution privacy and resource ownership | `load_snapshot_for_principal` fences snapshot queries; `snapshot_lookup_is_principal_fenced_and_hides_foreign_execution` proves foreign snapshots return no result. Existing control/event, workspace, and artifact paths additionally check the stored principal and, where required, lease/generation. |
| Secret-negative Debug/error handling | `execution_spec_debug_redacts_credentials_and_command_arguments` covers argv, environment, stdin, execution metadata, stdout, and event metadata. `runner_request_debug_redacts_command_and_inputs` covers runner values. Client endpoint credentials and paths are excluded; `endpoint_rejects_credentials_and_non_tls_schemes` and `client_error_debug_redacts_remote_message` verify URL and API error redaction. `tls_configuration_diagnostics_redact_certificate_paths` checks startup diagnostics do not display a key path. |
| Threat actors and mitigations documented | `plans/security/threat-model.md` covers unauthenticated and unauthorized peers, malicious controllers/processes, hostile workspaces, proxies, stale controllers, local users, and dependency compromise; it records assets, boundaries, route policy, secret classifications, and residual limits. |
| Unsafe TLS verification and logging guards | Source inspection found no unsafe TLS verification switches and no direct `tracing`, `println!`, or `eprintln!` calls under `crates/`. TLS validation remains delegated to EggServe/Eggfetch configurations and mTLS verification. |

## Verification actually run

Host: Linux x86_64; repository Rust toolchain 1.89.0.

- `cargo fmt --all` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed with no warnings.
- `cargo test --workspace --all-targets` — passed (55 tests across 4 suites; RTK summary, 7.15s).
- `git diff --check` — passed.
- Static source searches for unsafe TLS verification switches and direct logging macros under `crates/` — no matches.

No Windows/macOS runtime qualification, independent dependency audit, hosted
certificate lifecycle test, or external penetration test was performed.

## Compatibility and security review

- No HTTP route or request success shape was added. Unknown fields now fail closed on mutation/read request DTOs; callers sending undocumented fields must remove them.
- `Authorizer` keeps its existing required operation-level method and adds a default resource-aware method, preserving current closure policies.
- `ClientError` and `NodeStartError` now hide nested transport/configuration diagnostics from Display and Debug. Callers should use stable status/code categories rather than parse remote error prose.
- The raw BlobRead capability remains digest-based. A principal granted broad BlobRead can read any known digest; deployments needing per-digest policy should override the resource-aware authorizer. This is an explicit capability boundary, not an ownership claim.
- Process stdout/stderr and artifacts remain intentionally available to the authorized execution owner. Consumers must not log those application-controlled payloads without their own redaction policy.
- No full audit database or authorization-decision ID is introduced. The execution store already records principal ID, canonical request version/digest, execution ID/generation, and lease hash; node ID is configured on the node/status surface. Opaque caller provenance remains a bounded future extension.
- No high/medium finding in the scoped M001 review blocks closure. The broader remote-execution security qualification remains incomplete until M002/M003 and later adversarial qualification.

## Registry/roadmap disposition

Security M001 is closed. Security M002 is promoted to active because its
direct Foundation M002 and Workspace M002 dependencies were already closed.
Security M003 remains blocked on M002. Operations M001 and CodeGG M001 remain
blocked on Security M003; Operations M002 also waits on the stable Eggup
consumer interface.
