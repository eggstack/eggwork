# Security M001 — Authorization, Redaction, and Threat-Model Foundation

Status: closed

Dependency evidence: `plans/closure/foundation-execution-core/001-status.md` and `plans/closure/control-plane-protocol/001-status.md`

Closure evidence: `plans/closure/security-isolation-resource/001-status.md`

Source roadmap:

- `plans/subsystems/security-isolation-resource-roadmap.md`

## 1. Objective

Define and enforce the node authorization and secret-safety boundary before richer filesystem/resource controls are added.

## 2. Threat actors

The threat model must cover at least:

- unauthenticated network peer;
- authenticated but unauthorized principal;
- malicious/compromised controller;
- malicious executed process;
- hostile workspace filenames/content/symlinks;
- malicious proxy/intermediary;
- compromised/stale controller attempting generation reuse;
- local unprivileged user on a multi-user node;
- supply-chain/dependency compromise within realistic project scope.

## 3. Authorization model

Define operation descriptors/capabilities for:

- capabilities/status read;
- execute;
- execution observe/result;
- cancel;
- lease renew/takeover if supported;
- blob upload/read;
- workspace create/delete;
- artifact read/delete;
- drain/admin;
- future relay/PTY.

Authorization consumes server-resolved PrincipalContext plus operation/resource IDs. Request payload cannot choose principal.

Denial is before side effects whenever the requested operation has not already been accepted under an existing execution lease.

## 4. Secret classification/redaction

Review and classify:

- TLS private key paths/material;
- bearer/bootstrap tokens if any;
- environment values;
- route credentials;
- proxy credentials;
- authorization headers;
- workspace content;
- artifact content;
- peer certificate chain;
- request metadata.

Debug/Display/error/event/log surfaces must use structural redaction, not regex-only cleanup after formatting.

## 5. Audit/provenance seam

Without building a full audit database, define bounded attribution fields needed by future consumers:

- principal ID;
- node ID;
- execution ID/generation;
- request digest/version;
- decision/authorization result reference if available;
- caller-provided opaque provenance keys under strict bounds.

No hidden reasoning or secrets.

## 6. Static checks

Add guards/tests that:

- network execute cannot bypass authorization hook;
- payload structs have no authoritative principal/role/capability field;
- secret-bearing types do not derive unsafe Debug;
- unsafe TLS verification flags are absent from qualified production profile or explicitly gated.

## 7. Tests

- authz allow/deny matrix;
- deny zero process/workspace/blob side effects;
- forged principal JSON ignored/rejected;
- secret-negative serialization;
- proxy URI credentials;
- env secret;
- cert/key diagnostics;
- error propagation;
- malformed IDs/resource enumeration privacy as applicable.

## 8. Acceptance criteria

1. Every network operation has an authorization descriptor.
2. Denied mutation has zero side effect.
3. Trusted identity cannot originate from request JSON.
4. Secret-negative tests cover all known credential classes.
5. Threat model is documented and tied to mitigations.
6. No high/medium finding remains before declaring remote execution security-qualified.

## 9. Closure evidence

Create `plans/closure/security-isolation-resource/001-status.md` with authorization matrix and secret-negative evidence.
