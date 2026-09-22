# Foundation M001 Closure — Repository Bootstrap and Domain Contract

Source plan: `plans/implementation/foundation-execution-core/001-repository-bootstrap-and-domain-contract.md`  
Subsystem roadmap: `plans/subsystems/foundation-execution-core-roadmap.md`  
Reviewed baseline: `a5ac532`  
Implementation commit: `0de99cf9bf36b61090915223a6486b0893830459`

## Finding

M001 is closed. The repository now has a Rust 1.89 workspace with dependency ownership separated across core, runner, client, and server crates. Core contains validated transport-neutral domain types, bounded requests/events, typed lifecycle/error states, and versioned request digesting. No listener or process spawning was introduced.

## Requirement-to-evidence matrix

| Requirement | Evidence |
|---|---|
| Workspace and ownership graph | `Cargo.toml`; core has no adapter dependency; runner depends on core; server depends on core and runner; client depends on core. `cargo check --workspace` passed. |
| Validated identities and digest | `NodeId`, `PrincipalId`, `ExecutionId`, `LeaseId`, `WorkspaceId`, `ArtifactId`, `ExecutionGeneration`, `EventSequence`, `BlobDigest`; unit test covers valid/invalid IDs, generation ordering, SHA-256 digest format/value. |
| Bounded request and event fields | Central constants and validators cover argv, environment, paths, metadata, stdin, timeout, outputs, event payload, output capture, and capabilities. Tests cover empty/oversized argv, traversal/absolute paths, NUL environment values, and round trips. |
| Serialization and compatibility baseline | Serde derives, explicit schema version and `Versioned<T>` envelope; `ExecutionSpec` JSON round trip passed. Additive unknown fields are not promised for these closed Rust structs and no public wire schema is frozen. |
| Scheduler-free ownership | No placement/scheduling API exists. `architecture/execution-ownership.md` records caller-owned selection and local immediate admission boundary. |
| Architecture docs and CI | `architecture/{overview,domain,execution-ownership,protocol}.md`; CI runs fmt, Clippy, tests, and check. Security/contribution placeholders and `.gitignore` added. |

## Verification actually run

Using the pinned toolchain (`rustc 1.89.0 (29483883e 2025-08-04)`):

- `cargo fmt --all -- --check` — passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace --all-targets` — passed (4 suites reported by RTK; all tests passed).
- `cargo check --workspace` — passed.

## Review and residuals

- Dependency review: only Serde, serde_json, thiserror, sha2, and hex are used; no transport or platform dependency added.
- Platform qualification: Linux host only; M001 has no platform-specific behavior.
- Security review: no secrets or executable behavior in M001. Deserialization into request values is followed by explicit `validate()` before use; callers must preserve this boundary.
- Compatibility review: initial 0.1 Rust domain model only; no stable wire compatibility claim.
- Unresolved findings: none at high or medium severity.
- Disposition: **closed**.

## Registry and next-plan disposition

The foundation roadmap and registry mark M001 closed. M002 is directly unblocked and active. Its required CodeGG lifecycle reference was inspected at current head `28b4695661d463dd1675d045ac6299c5fbc9ea31`; the bounded argv, environment, output capture, process-session, timeout/cancellation, and sandbox hook concepts are available for generalization without CodeGG domain imports. No M002 dependency blocker was found.
